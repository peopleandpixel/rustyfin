# Rustyfin Plugin-Entwicklung

Dieses Dokument beschreibt, wie man externe Plugins für Rustyfin entwickelt.

## Inhaltsverzeichnis

1. [Überblick](#überblick)
2. [Plugin-Typen](#plugin-typen)
3. [Voraussetzungen](#voraussetzungen)
4. [Projekt-Struktur](#projekt-struktur)
5. [Metadaten-Provider](#metadaten-provider)
6. [Untertitel-Provider](#untertitel-provider)
7. [Benachrichtigungs-Provider](#benachrichtigungs-provider)
8. [Konfiguration](#konfiguration)
9. [FFI für dynamisches Laden](#ffi-für-dynamisches-laden)
10. [Bauen und Installieren](#bauen-und-installieren)
11. [Priorität](#priorität)
12. [Beispiel-Plugin](#beispiel-plugin)

## Überblick

Rustyfin unterstützt ein Plugin-System, das es ermöglicht, neue Metadaten-Quellen, Untertitel-Provider und Benachrichtigungskanäle hinzuzufügen, ohne den Core neu kompilieren zu müssen. Plugins können entweder **statisch** (als Crate im Workspace) oder **dynamisch** (als `.so`/`.dylib`/`.dll`) geladen werden.

## Plugin-Typen

| Typ | Beschreibung | Beispiele |
|-----|-------------|-----------|
| `MetadataProvider` | Externe Metadaten für Filme/Serien | TMDB, OMDb, TheTVDB, Trakt |
| `SubtitleProvider` | Untertitel-Suche und -Download | OpenSubtitles, Podnapisi |
| `NotificationProvider` | Push-Benachrichtigungen | ntfy, MQTT, Pushover, Webhook |
| `ImageProvider` | Zusätzliche Bildquellen | (noch nicht implementiert) |
| `AuthenticationProvider` | Authentifizierungs-Backends | (noch nicht implementiert) |
| `Task` | Geplante Hintergrundaufgaben | (noch nicht implementiert) |

## Voraussetzungen

- Rust 1.75+ (Edition 2021)
- Zugriff auf die `rustyfin-model` Crate (für Traits und Typen)

## Projekt-Struktur

Ein typisches Plugin-Projekt sieht so aus:

```
plugin-myplugin/
├── Cargo.toml
└── src/
    └── lib.rs
```

### Cargo.toml

```toml
[package]
name = "plugin-myplugin"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
rustyfin-model = { path = "../crates/rustyfin-model" }
reqwest = { version = "0.12", features = ["json"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tracing = "0.1"

[features]
dynamic = []
```

Wichtig:
- `crate-type = ["cdylib", "rlib"]` ermöglicht sowohl statisches als auch dynamisches Laden
- Das Feature `dynamic` aktiviert die FFI-Eintrittspunkte

## Metadaten-Provider

Ein Metadaten-Provider sucht externe Metadaten für Filme und Serien.

### Schritt 1: Plugin-Struktur definieren

```rust
use rustyfin_model::plugin::{Plugin, PluginType};
use std::sync::Mutex;

#[derive(Clone)]
pub struct MyPlugin {
    api_key: Mutex<Option<String>>,
}

impl Plugin for MyPlugin {
    fn id(&self) -> &str { "myplugin" }
    fn name(&self) -> &str { "My Metadata Provider" }
    fn version(&self) -> &str { env!("CARGO_PKG_VERSION") }
    fn description(&self) -> &str { "Fetches metadata from my source" }
    fn plugin_type(&self) -> PluginType { PluginType::MetadataProvider }

    fn set_configuration(&self, config: serde_json::Value) -> Result<(), String> {
        *self.api_key.lock().unwrap() = config
            .get("api_key")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        Ok(())
    }

    fn get_configuration(&self) -> Option<serde_json::Value> {
        Some(serde_json::json!({
            "api_key": self.api_key.lock().unwrap().as_deref()
        }))
    }

    fn as_any(&self) -> &dyn std::any::Any { self }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
}
```

### Schritt 2: Such-Funktionen implementieren

```rust
use rustyfin_model::metadata::{ExternalMetadata, MetadataImage};
use rustyfin_model::media::MediaType;

async fn search_movie(
    client: &reqwest::Client,
    api_key: &str,
    name: &str,
    year: Option<i32>,
) -> Option<ExternalMetadata> {
    // Implementiere die Suche nach Filmen
    // Gib ExternalMetadata zurück oder None
    todo!()
}

async fn search_tv(
    client: &reqwest::Client,
    api_key: &str,
    name: &str,
) -> Option<ExternalMetadata> {
    // Implementiere die Suche nach Serien
    todo!()
}

async fn get_images(
    client: &reqwest::Client,
    api_key: &str,
    tmdb_id: Option<i64>,
    tvdb_id: Option<i64>,
    media_type: MediaType,
) -> Vec<MetadataImage> {
    // Implementiere das Laden von Bildern
    Vec::new()
}
```

### Schritt 3: Registrar-Trait und Registrierungs-Funktion

```rust
use std::future::Future;
use std::pin::Pin;

pub trait MetadataProviderRegistrar {
    async fn register_metadata_provider<F1, F2, F3>(
        &self,
        plugin_id: String,
        provider_name: String,
        search_movie: F1,
        search_tv: F2,
        get_images: F3,
    ) where
        F1: Fn(&reqwest::Client, &str, Option<i32>) -> Pin<Box<dyn Future<Output = Option<ExternalMetadata>> + Send>> + Send + Sync + 'static,
        F2: Fn(&reqwest::Client, &str) -> Pin<Box<dyn Future<Output = Option<ExternalMetadata>> + Send>> + Send + Sync + 'static,
        F3: Fn(&reqwest::Client, Option<i64>, Option<i64>, MediaType) -> Pin<Box<dyn Future<Output = Vec<MetadataImage>> + Send>> + Send + Sync + 'static;
}

pub async fn register_myplugin_provider<P>(
    plugin: &MyPlugin,
    plugin_manager: &P,
) -> Result<(), String>
where
    P: MetadataProviderRegistrar,
{
    let api_key = plugin.api_key.lock().unwrap().clone()
        .ok_or("API key not configured")?;
    let plugin_id = plugin.id().to_string();

    plugin_manager
        .register_metadata_provider(
            plugin_id,
            "myplugin".to_string(),
            move |client, name, year| {
                let api_key = api_key.clone();
                let name = name.to_string();
                Box::pin(async move {
                    search_movie(client, &api_key, &name, year).await
                })
            },
            move |client, name| {
                let api_key = api_key.clone();
                let name = name.to_string();
                Box::pin(async move {
                    search_tv(client, &api_key, &name).await
                })
            },
            move |client, tmdb_id, tvdb_id, media_type| {
                let api_key = api_key.clone();
                Box::pin(async move {
                    get_images(client, &api_key, tmdb_id, tvdb_id, media_type).await
                })
            },
        )
        .await;

    Ok(())
}
```

### Schritt 4: FFI-Eintrittspunkt (optional, für dynamisches Laden)

```rust
#[cfg(feature = "dynamic")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rustyfin_plugin_create() -> Box<dyn Plugin> {
    Box::new(MyPlugin {
        api_key: Mutex::new(None),
    })
}
```

## Untertitel-Provider

Ähnlich wie Metadaten-Provider, aber mit `SubtitleProviderRegistrar`:

```rust
use rustyfin_model::plugin::{Plugin, PluginType, SubtitleResult};
use rustyfin_model::media::MediaItem;

pub trait SubtitleProviderRegistrar {
    async fn register_subtitle_provider<F1, F2>(
        &self,
        plugin_id: String,
        search_subtitles: F1,
        download_subtitle: F2,
    ) where
        F1: Fn(&reqwest::Client, &MediaItem, &str) -> Pin<Box<dyn Future<Output = Vec<SubtitleResult>> + Send>> + Send + Sync + 'static,
        F2: Fn(&reqwest::Client, &str, &std::path::Path) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> + Send + Sync + 'static;
}
```

## Benachrichtigungs-Provider

```rust
use rustyfin_model::plugin::{Plugin, PluginType, NotificationPriority};

pub trait NotificationProviderRegistrar {
    async fn register_notification_provider<F>(
        &self,
        plugin_id: String,
        send_notification: F,
    ) where
        F: Fn(&str, &str, NotificationPriority) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> + Send + Sync + 'static;
}
```

## Konfiguration

Plugins speichern ihre Konfiguration in der Datenbank, nicht in `config.json`. Die Konfiguration wird als JSON gespeichert und kann über die WebGUI oder die API bearbeitet werden.

### API-Endpunkte

- `GET /Plugins/{id}/Configuration` – Konfiguration lesen
- `POST /Plugins/{id}/Configuration` – Konfiguration schreiben
- `GET /Plugins/{id}/Priority` – Priorität lesen
- `PUT /Plugins/{id}/Priority` – Priorität setzen

### Konfigurations-Beispiel

```json
{
    "api_key": "mein-api-key",
    "base_url": "https://api.example.com"
}
```

## FFI für dynamisches Laden

Für dynamisches Laden muss das Plugin als `cdylib` kompiliert und die Funktion `rustyfin_plugin_create()` exportieren:

```bash
cargo build --release --features dynamic --lib -p plugin-myplugin
```

Die resultierende Datei (`libplugin_myplugin.so` auf Linux, `libplugin_myplugin.dylib` auf macOS, `plugin_myplugin.dll` auf Windows) wird in `data/plugins/` kopiert.

## Bauen und Installieren

### Statisch (Built-in)

1. Plugin-Crate ins Workspace aufnehmen (`Cargo.toml`)
2. In `main.rs` registrieren
3. Server neu kompilieren

### Dynamisch

1. Plugin mit `--features dynamic` bauen
2. `.so`/`.dylib`/`.dll` in `data/plugins/` kopieren
3. Server neu starten – das Plugin wird automatisch geladen

## Priorität

Jedes Plugin hat eine Priorität (Standard: 100). Niedrigere Werte werden zuerst verarbeitet. Die Priorität kann über die WebGUI oder die API geändert werden:

```bash
# Priorität auf 50 setzen (höhere Priorität)
curl -X PUT http://localhost:8080/Plugins/tmdb/Priority \
  -H "Content-Type: application/json" \
  -d '{"priority": 50}'

# Priorität lesen
curl http://localhost:8080/Plugins/tmdb/Priority
# {"priority": 50}
```

Bei Metadaten-Providern wird der Provider mit der höchsten Priorität (niedrigster Wert) zuerst abgefragt. Sobald ein Provider Metadaten liefert, werden die folgenden Provider übersprungen.

## Beispiel-Plugin

Siehe die bestehenden Plugins als Referenz:

- `crates/plugin-tmdb/` – Metadaten von TMDB
- `crates/plugin-omdb/` – Metadaten von OMDb
- `crates/plugin-opensubtitles/` – Untertitel von OpenSubtitles
- `crates/plugin-ntfy/` – Benachrichtigungen via ntfy
- `crates/plugin-webhook/` – Generische Webhook-Benachrichtigungen
