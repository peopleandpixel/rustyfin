//! The built-in notification plugins: a config saved while the server runs takes effect at once,
//! and the server's events reach the enabled providers.

use std::sync::{Arc, Mutex};

use rustyfin_core::plugin_manager::{NotificationEvent, PluginManager};
use rustyfin_model::plugin::PluginStatus;

/// A stub webhook receiver that keeps every body it gets.
async fn receiver() -> (String, Arc<Mutex<Vec<String>>>) {
    let bodies: Arc<Mutex<Vec<String>>> = Arc::default();
    let kept = bodies.clone();
    let handle = move |body: String| {
        let kept = kept.clone();
        async move {
            kept.lock().unwrap().push(body);
            axum::http::StatusCode::OK
        }
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let app = axum::Router::new().fallback(handle);
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (origin, bodies)
}

/// A plugin manager over a fresh database, with the built-in plugins installed.
async fn manager() -> (PluginManager, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("rf-plugins-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let pool = rustyfin_db::init_db(dir.join("test.db").to_str().unwrap())
        .await
        .unwrap();
    let pm = PluginManager::new(&dir).with_database(pool);
    rustyfin_core::plugin_manager::install_other_plugins(&pm).await;
    (pm, dir)
}

/// A webhook URL saved while the server runs is used by the next notification, without a
/// restart: before it, the enabled plugin sends nothing; after it, the event reaches the URL.
#[tokio::test]
async fn r3meta_a_saved_webhook_config_is_used_at_once() {
    let (origin, bodies) = receiver().await;
    let (pm, dir) = manager().await;
    pm.set_plugin_status("webhook", PluginStatus::Active)
        .await
        .unwrap();

    let sent = pm
        .send_notification(
            NotificationEvent::TaskFailed,
            "Scan failed",
            "no config yet",
        )
        .await;
    assert_eq!(sent, 0);
    assert!(bodies.lock().unwrap().is_empty());

    pm.set_configuration(
        "webhook",
        serde_json::json!({"endpoints": [{"url": format!("{origin}/hook")}]}),
    )
    .await
    .unwrap();
    let sent = pm
        .send_notification(
            NotificationEvent::NewLibraryContent,
            "New content added",
            "3 items have been added to your media library",
        )
        .await;
    assert_eq!(sent, 1);
    let got = bodies.lock().unwrap().clone();
    assert_eq!(got.len(), 1);
    assert!(got[0].contains("New content added"), "{got:?}");

    // A disabled plugin is not asked
    pm.set_plugin_status("webhook", PluginStatus::Disabled)
        .await
        .unwrap();
    let sent = pm
        .send_notification(NotificationEvent::TaskFailed, "Scan failed", "off")
        .await;
    assert_eq!(sent, 0);
    assert_eq!(bodies.lock().unwrap().len(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}

/// `notify` sends in the background: the caller doesn't wait, and the event still arrives.
#[tokio::test]
async fn r3meta_notify_reaches_an_enabled_provider() {
    let (origin, bodies) = receiver().await;
    let (pm, dir) = manager().await;
    pm.set_configuration(
        "webhook",
        serde_json::json!({"endpoints": [{"url": format!("{origin}/hook")}]}),
    )
    .await
    .unwrap();
    pm.set_plugin_status("webhook", PluginStatus::Active)
        .await
        .unwrap();
    pm.notify(
        NotificationEvent::VideoPlayback,
        "Playback started",
        "Dana is playing Lantern Hour",
    );
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while bodies.lock().unwrap().is_empty() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "no notification arrived"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(bodies.lock().unwrap()[0].contains("Dana is playing Lantern Hour"));
    std::fs::remove_dir_all(dir).unwrap();
}
