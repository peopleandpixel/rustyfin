<p align="center">
  <img src="rustyfin.jpeg" alt="Rustyfin Logo" width="120"/>
</p>

# Architektur

## Projektstruktur

Rustyfin ist als Cargo-Workspace mit mehreren Crates organisiert:

```
rustyfin/
├── Cargo.toml                  # Workspace-Definition
├── crates/
│   ├── rustyfin-server/        # Einstiegspunkt und Server-Bootstrap
│   ├── rustyfin-api/           # Axum-Routes und HTTP-Handler
│   ├── rustyfin-auth/          # Authentifizierung (Argon2, JWT, Quick Connect)
│   ├── rustyfin-core/          # Geschäftslogik (Library-Scan, Metadaten, Bilder)
│   ├── rustyfin-db/            # Datenbankschicht (SQLite via sqlx)
│   ├── rustyfin-model/         # Gemeinsame Datenmodelle und DTOs
│   └── rustyfin-cli/           # Kommandozeilen-Utilities
└── docs/                       # Dokumentation
```

## Crate-Übersicht

### rustyfin-server

Einstiegspunkt der Anwendung. Initialisiert die Datenbank, erstellt den Application-State und startet den Axum-HTTP-Server.

### rustyfin-api

Enthält alle HTTP-Routes und Handler, organisiert nach Funktionsbereichen:

- `routes/system.rs` – System-Informationen und -Verwaltung
- `routes/auth.rs` – Authentifizierung
- `routes/users.rs` – Benutzerverwaltung
- `routes/library.rs` – Bibliotheksverwaltung
- `routes/items.rs` – Medien-Browsing
- `routes/images.rs` – Bildverarbeitung
- `routes/streaming.rs` – Video/Audio-Streaming
- `routes/playback.rs` – Wiedergabestatus
- `routes/search.rs` – Suche
- `routes/collections.rs` – Sammlungen
- `routes/session.rs` – Session-Verwaltung
- `routes/scheduled_tasks.rs` – Geplante Aufgaben
- `routes/plugins.rs` – Plugin-System
- `routes/startup.rs` – Setup-Wizard
- `routes/quick_connect.rs` – Quick Connect
- `routes/subtitle.rs` – Untertitel
- `routes/transcode.rs` – Transkodierung
- `routes/dlna.rs` – DLNA
- `routes/persons.rs` – Personen
- `routes/activity.rs` – Aktivitätslog

### rustyfin-auth

Authentifizierungs-Logik:

- **Argon2** – Passwort-Hashing
- **JWT** – Token-basierte Authentifizierung
- **Quick Connect** – Code-basierte Anmeldung
- **API-Key** – Statische Schlüssel für Integrationen

### rustyfin-core

Kerngeschäftslogik:

- **Library-Scanner** – Dateisystem scannen und Medien erkennen
- **Metadaten-Provider** – TMDb, OMDb, NFO-Parser
- **Image-Proxy** – Bildcaching und -skalierung

### rustyfin-db

Datenbankschicht mit SQLite:

- **Migrationen** – Schema-Versionierung
- **Repositories** – Datenzugriffsfunktionen für alle Entitäten

### rustyfin-model

Gemeinsame Datenstrukturen (DTOs), die zwischen den Crates geteilt werden. Definiert die Jellyfin-kompatiblen JSON-Strukturen.

### rustyfin-cli

Kommandozeilen-Tools und Hilfsfunktionen.

## Technologie-Stack

| Komponente | Technologie |
|---|---|
| Sprache | Rust (Edition 2024) |
| Async Runtime | Tokio |
| Web Framework | Axum |
| Datenbank | SQLite (via sqlx) |
| Passwort-Hashing | Argon2 |
| JWT | jsonwebtoken |
| Bildverarbeitung | image crate |
| HTTP-Client | reqwest |
| Serialisierung | serde / serde_json |

## Architektur-Diagramm

```
┌─────────────────────────────────────────────┐
│                Jellyfin-Clients             │
│         (Web, Android, iOS, TV)             │
└──────────────────┬──────────────────────────┘
                   │ HTTP/JSON
┌──────────────────▼──────────────────────────┐
│              rustyfin-server                │
│         (Tokio + Axum HTTP-Server)          │
├─────────────────────────────────────────────┤
│              rustyfin-api                   │
│           (Routes & Handler)                │
├──────────┬──────────────┬───────────────────┤
│rustyfin- │ rustyfin-    │   rustyfin-       │
│  auth    │   core       │     db            │
│(JWT,     │(Scanner,     │(SQLite,           │
│ Argon2,  │ Metadaten,   │ Repositories,     │
│ QConnect)│ Bilder)      │ Migrationen)      │
├──────────┴──────────────┴───────────────────┤
│              rustyfin-model                 │
│          (Gemeinsame DTOs)                  │
└─────────────────────────────────────────────┘
```
