use axum::Json;
use axum::Router;
use axum::extract::{Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use std::collections::HashMap;

use rustyfin_core::dlna;
use rustyfin_model::media::MediaType;

use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/dlna/description.xml", get(description))
        .route("/dlna/ContentDirectory.xml", get(content_directory_scpd))
        .route("/dlna/ConnectionManager.xml", get(connection_manager_scpd))
        .route(
            "/dlna/control/ContentDirectory",
            post(content_directory_control),
        )
        .route(
            "/dlna/control/ConnectionManager",
            post(connection_manager_control),
        )
        .route("/Dlna/Profile", get(get_profile))
        .route("/Dlna/Profile/DeviceDetection", post(device_detection))
        .route("/Dlna/Profile/Device", get(get_device_profile))
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
            let libraries = rustyfin_db::repo_library::get_all_libraries(&state.db_ro).await?;
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
                if let Ok(Some(item)) = rustyfin_db::repo_media::get_item_by_id(&state.db, id).await
                {
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
                    &state.db_ro,
                    parent_id,
                    start_index,
                    requested_count,
                )
                .await?;
                let total =
                    rustyfin_db::repo_media::count_children(&state.db_ro, parent_id).await?;
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

/// GET /Dlna/Profile — Returns the default DLNA profile.
async fn get_profile() -> Result<Json<rustyfin_model::dlna_profile::DlnaProfile>, ApiError> {
    let profile = default_dlna_profile();
    Ok(Json(profile))
}

/// POST /Dlna/Profile/DeviceDetection — Detects device from user agent.
async fn device_detection(
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<rustyfin_model::dlna_profile::DlnaDeviceDetection>, ApiError> {
    let user_agent = params.get("UserAgent").cloned().unwrap_or_default();
    let detection = detect_device(&user_agent);
    Ok(Json(detection))
}

/// GET /Dlna/Profile/Device — Returns device-specific DLNA profile.
async fn get_device_profile(
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<rustyfin_model::dlna_profile::DlnaProfile>, ApiError> {
    let user_agent = params.get("UserAgent").cloned().unwrap_or_default();
    let _detection = detect_device(&user_agent);
    // Return the default profile; device-specific profiles can be added later
    let profile = default_dlna_profile();
    Ok(Json(profile))
}

/// Build the default DLNA profile matching Jellyfin's format.
fn default_dlna_profile() -> rustyfin_model::dlna_profile::DlnaProfile {
    use rustyfin_model::dlna_profile::*;

    DlnaProfile {
        name: "Default".to_string(),
        device_profile_xml: None,
        direct_play_profiles: vec![
            DlnaStreamProfile {
                container: Some("mp4,mkv,webm".to_string()),
                profile_type: Some("Video".to_string()),
                protocol: Some("http".to_string()),
                estimate_content_length: Some(false),
                enable_mpegts_m2ts_mode: Some(false),
                video_codec: None,
                audio_codec: None,
            },
            DlnaStreamProfile {
                container: Some("mp3,aac,flac,ogg,wma".to_string()),
                profile_type: Some("Audio".to_string()),
                protocol: Some("http".to_string()),
                estimate_content_length: Some(false),
                enable_mpegts_m2ts_mode: Some(false),
                video_codec: None,
                audio_codec: None,
            },
        ],
        transcoding_profiles: vec![
            DlnaStreamProfile {
                container: Some("ts".to_string()),
                profile_type: Some("Video".to_string()),
                protocol: Some("hls".to_string()),
                estimate_content_length: Some(false),
                enable_mpegts_m2ts_mode: Some(true),
                video_codec: Some("h264".to_string()),
                audio_codec: Some("aac,mp3".to_string()),
            },
            DlnaStreamProfile {
                container: Some("mp3".to_string()),
                profile_type: Some("Audio".to_string()),
                protocol: Some("http".to_string()),
                estimate_content_length: Some(false),
                enable_mpegts_m2ts_mode: Some(false),
                video_codec: None,
                audio_codec: Some("mp3".to_string()),
            },
        ],
        container_profiles: vec![],
        response_profiles: vec![DlnaResponseProfile {
            mime_type: "audio/mp4".to_string(),
            container: Some("aac".to_string()),
            profile_type: "Audio".to_string(),
            org_path: None,
        }],
        codec_profiles: vec![],
        subtitle_profiles: vec![
            DlnaSubtitleProfile {
                format: Some("srt".to_string()),
                method: "External".to_string(),
                didl_mode: None,
            },
            DlnaSubtitleProfile {
                format: Some("ass".to_string()),
                method: "Encode".to_string(),
                didl_mode: None,
            },
        ],
    }
}

/// Detect device information from a user agent string.
fn detect_device(user_agent: &str) -> rustyfin_model::dlna_profile::DlnaDeviceDetection {
    use rustyfin_model::dlna_profile::DlnaDeviceDetection;

    // Try to identify common devices
    let (friendly_name, model_name, model_number, manufacturer) = if user_agent.contains("Jellyfin")
    {
        ("Jellyfin Client", "Jellyfin", "1.0.0", "Jellyfin")
    } else if user_agent.contains("Swiftfin") {
        ("Swiftfin", "Swiftfin", "1.0.0", "Swiftfin")
    } else if user_agent.contains("Kodi") {
        ("Kodi", "Kodi", "1.0.0", "XBMC Foundation")
    } else if user_agent.contains("PS3") || user_agent.contains("PLAYSTATION") {
        ("PlayStation", "PlayStation", "1.0.0", "Sony")
    } else if user_agent.contains("Xbox") {
        ("Xbox", "Xbox", "1.0.0", "Microsoft")
    } else if user_agent.contains("Android") {
        ("Android Device", "Android", "1.0.0", "Google")
    } else if user_agent.contains("iPhone") || user_agent.contains("iPad") {
        ("Apple Device", "iOS", "1.0.0", "Apple")
    } else if user_agent.contains("DLNA") || user_agent.contains("UPnP") {
        ("DLNA Device", "DLNA", "1.0.0", "Unknown")
    } else {
        ("Unknown Device", "Unknown", "1.0.0", "Unknown")
    };

    DlnaDeviceDetection {
        user_agent: user_agent.to_string(),
        friendly_name: friendly_name.to_string(),
        model_name: model_name.to_string(),
        model_number: model_number.to_string(),
        serial_number: String::new(),
        manufacturer: manufacturer.to_string(),
        manufacturer_url: String::new(),
        device_id: uuid::Uuid::new_v4().to_string(),
        icon_url: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustyfin_model::media::MediaType;

    // ── xml_escape ──

    #[test]
    fn test_xml_escape_ampersand() {
        assert_eq!(xml_escape("rock & roll"), "rock &amp; roll");
    }

    #[test]
    fn test_xml_escape_less_than() {
        assert_eq!(xml_escape("a < b"), "a &lt; b");
    }

    #[test]
    fn test_xml_escape_greater_than() {
        assert_eq!(xml_escape("a > b"), "a &gt; b");
    }

    #[test]
    fn test_xml_escape_quotes() {
        assert_eq!(xml_escape(r#"say "hello""#), r#"say &quot;hello&quot;"#);
    }

    #[test]
    fn test_xml_escape_all_special_chars() {
        assert_eq!(xml_escape(r#"a & "b" < c > d"#), r#"a &amp; &quot;b&quot; &lt; c &gt; d"#);
    }

    #[test]
    fn test_xml_escape_empty_string() {
        assert_eq!(xml_escape(""), "");
    }

    #[test]
    fn test_xml_escape_no_special_chars() {
        assert_eq!(xml_escape("Hello World 123!"), "Hello World 123!");
    }

    // ── upnp_class ──

    #[test]
    fn test_upnp_class_container() {
        assert_eq!(
            upnp_class(&MediaType::Movie, true),
            "object.container.storageFolder"
        );
    }

    #[test]
    fn test_upnp_class_movie_item() {
        assert_eq!(
            upnp_class(&MediaType::Movie, false),
            "object.item.videoItem.movie"
        );
    }

    #[test]
    fn test_upnp_class_episode_item() {
        assert_eq!(
            upnp_class(&MediaType::Episode, false),
            "object.item.videoItem.movie"
        );
    }

    #[test]
    fn test_upnp_class_audio_item() {
        assert_eq!(
            upnp_class(&MediaType::Audio, false),
            "object.item.audioItem.musicTrack"
        );
    }

    #[test]
    fn test_upnp_class_photo_item() {
        assert_eq!(
            upnp_class(&MediaType::Photo, false),
            "object.item.imageItem.photo"
        );
    }

    #[test]
    fn test_upnp_class_unknown_item() {
        assert_eq!(upnp_class(&MediaType::Unknown, false), "object.item");
    }

    #[test]
    fn test_upnp_class_series_container() {
        assert_eq!(
            upnp_class(&MediaType::Series, true),
            "object.container.storageFolder"
        );
    }

    // ── build_browse_response ──

    #[test]
    fn test_build_browse_response_empty() {
        let result = build_browse_response("", 0, 0);
        assert!(result.contains("<Result>"));
        assert!(result.contains("<NumberReturned>0</NumberReturned>"));
        assert!(result.contains("<TotalMatches>0</TotalMatches>"));
        assert!(result.contains("<UpdateID>1</UpdateID>"));
    }

    #[test]
    fn test_build_browse_response_with_items() {
        let result = build_browse_response("<item/>", 1, 10);
        // The item is XML-escaped inside the DIDL-Lite wrapper
        assert!(result.contains("&lt;item/&gt;"));
        assert!(result.contains("<NumberReturned>1</NumberReturned>"));
        assert!(result.contains("<TotalMatches>10</TotalMatches>"));
    }

    #[test]
    fn test_build_browse_response_escapes_didl() {
        let result = build_browse_response("<item><dc:title>Test</dc:title></item>", 1, 1);
        // DIDL-Lite wrapper should be XML-escaped inside <Result>
        assert!(result.contains("&lt;DIDL-Lite"));
    }

    // ── extract_xml_value ──

    #[test]
    fn test_extract_xml_value_simple() {
        let xml = "<root><name>John</name></root>";
        assert_eq!(extract_xml_value(xml, "name"), Some("John".to_string()));
    }

    #[test]
    fn test_extract_xml_value_missing_tag() {
        let xml = "<root><name>John</name></root>";
        assert_eq!(extract_xml_value(xml, "age"), None);
    }

    #[test]
    fn test_extract_xml_value_empty_content() {
        let xml = "<root><name></name></root>";
        assert_eq!(extract_xml_value(xml, "name"), Some("".to_string()));
    }

    #[test]
    fn test_extract_xml_value_nested_tags() {
        let xml = "<root><name><inner>John</inner></name></root>";
        assert_eq!(
            extract_xml_value(xml, "name"),
            Some("<inner>John</inner>".to_string())
        );
    }

    #[test]
    fn test_extract_xml_value_empty_xml() {
        assert_eq!(extract_xml_value("", "name"), None);
    }

    #[test]
    fn test_extract_xml_value_with_special_chars() {
        let xml = "<root><url>http://example.com?a=1&b=2</url></root>";
        assert_eq!(
            extract_xml_value(xml, "url"),
            Some("http://example.com?a=1&b=2".to_string())
        );
    }

    // ── parse_port ──

    #[test]
    fn test_parse_port_standard() {
        assert_eq!(parse_port("0.0.0.0:8096"), 8096);
    }

    #[test]
    fn test_parse_port_loopback() {
        assert_eq!(parse_port("127.0.0.1:8080"), 8080);
    }

    #[test]
    fn test_parse_port_default_when_missing() {
        assert_eq!(parse_port("0.0.0.0"), 8096);
    }

    #[test]
    fn test_parse_port_invalid_port() {
        assert_eq!(parse_port("0.0.0.0:abc"), 8096);
    }

    #[test]
    fn test_parse_port_empty_string() {
        assert_eq!(parse_port(""), 8096);
    }

    #[test]
    fn test_parse_port_ipv6() {
        assert_eq!(parse_port("[::1]:8096"), 8096);
    }

    // ── detect_device ──

    #[test]
    fn test_detect_device_jellyfin() {
        let detection = detect_device("Jellyfin/10.8.0");
        assert_eq!(detection.friendly_name, "Jellyfin Client");
        assert_eq!(detection.manufacturer, "Jellyfin");
    }

    #[test]
    fn test_detect_device_swiftfin() {
        let detection = detect_device("Swiftfin/2.0.0");
        assert_eq!(detection.friendly_name, "Swiftfin");
        assert_eq!(detection.manufacturer, "Swiftfin");
    }

    #[test]
    fn test_detect_device_kodi() {
        let detection = detect_device("Kodi/19.0");
        assert_eq!(detection.friendly_name, "Kodi");
        assert_eq!(detection.manufacturer, "XBMC Foundation");
    }

    #[test]
    fn test_detect_device_ps3() {
        let detection = detect_device("PLAYSTATION 3");
        assert_eq!(detection.friendly_name, "PlayStation");
        assert_eq!(detection.manufacturer, "Sony");
    }

    #[test]
    fn test_detect_device_xbox() {
        let detection = detect_device("Xbox One");
        assert_eq!(detection.friendly_name, "Xbox");
        assert_eq!(detection.manufacturer, "Microsoft");
    }

    #[test]
    fn test_detect_device_android() {
        let detection = detect_device("Mozilla/5.0 (Linux; Android 12)");
        assert_eq!(detection.friendly_name, "Android Device");
        assert_eq!(detection.manufacturer, "Google");
    }

    #[test]
    fn test_detect_device_iphone() {
        let detection = detect_device("Mozilla/5.0 (iPhone; CPU iPhone OS 16_0)");
        assert_eq!(detection.friendly_name, "Apple Device");
        assert_eq!(detection.manufacturer, "Apple");
    }

    #[test]
    fn test_detect_device_ipad() {
        let detection = detect_device("Mozilla/5.0 (iPad; CPU OS 16_0)");
        assert_eq!(detection.friendly_name, "Apple Device");
        assert_eq!(detection.manufacturer, "Apple");
    }

    #[test]
    fn test_detect_device_dlna() {
        let detection = detect_device("DLNA 1.5");
        assert_eq!(detection.friendly_name, "DLNA Device");
        assert_eq!(detection.manufacturer, "Unknown");
    }

    #[test]
    fn test_detect_device_unknown() {
        let detection = detect_device("SomeRandomAgent/1.0");
        assert_eq!(detection.friendly_name, "Unknown Device");
        assert_eq!(detection.manufacturer, "Unknown");
    }

    #[test]
    fn test_detect_device_empty() {
        let detection = detect_device("");
        assert_eq!(detection.friendly_name, "Unknown Device");
    }
}
