# Rustyfin – Tasks bis zur Jellyfin-Kompatibilität

## Phase 1: API-Kompatibilität (Grundlagen)

### System & Server
- [x] `GET /System/Info/Public` – Öffentliche Server-Infos
- [x] `GET /System/Info` – Vollständige Server-Infos (authentifiziert)
- [x] `POST /System/Shutdown` – Server herunterfahren
- [x] `POST /System/Restart` – Server neustarten
- [x] `GET /System/Configuration` – Server-Konfiguration abrufen
- [x] `POST /System/Configuration` – Server-Konfiguration ändern
- [x] `GET /System/Logs` – Server-Logs abrufen
- [x] `GET /System/Ping` – Ping-Endpoint

### Authentifizierung & Benutzer
- [x] `POST /Users/AuthenticateByName` – Login
- [x] `GET /Users` – Benutzerliste
- [x] `GET /Users/Me` – Aktueller Benutzer
- [x] `GET /Users/{id}` – Benutzer nach ID
- [x] `POST /Users/New` – Benutzer erstellen
- [x] `POST /Users/{id}/Delete` – Benutzer löschen
- [x] `POST /Users/{id}/Password` – Passwort ändern
- [x] `POST /Users/{id}/Policy` – Benutzerrichtlinien setzen (IsAdmin, etc.)
- [x] `POST /Users/{id}/Configuration` – Benutzereinstellungen
- [x] `GET /Users/Public` – Öffentliche Benutzerliste (Login-Screen)
- [x] `POST /Users/ForgotPassword` – Passwort vergessen (Stub)
- [x] API-Key-Authentifizierung (neben JWT, via RUSTYFIN_API_KEY)
- [x] Quick Connect Support (`/QuickConnect/Enabled`, `/Initiate`, `/Connect`, `/Authorize`)

### Bibliothek & Medien
- [x] `GET /Library/VirtualFolders` – Bibliotheken auflisten
- [x] `POST /Library/VirtualFolders` – Bibliothek erstellen
- [x] `DELETE /Library/VirtualFolders` – Bibliothek löschen
- [x] `POST /Library/VirtualFolders/LibraryOptions` – Bibliothek-Scan
- [x] `POST /Library/Refresh` – Alle Bibliotheken neu scannen
- [x] `GET /Items` – Medien abfragen (Paginierung, Sortierung, Filter)
- [x] `GET /Items/{id}` – Einzelnes Medium abrufen
- [x] `GET /Items/{id}/Similar` – Ähnliche Medien
- [x] `DELETE /Items/{id}` – Medium löschen
- [x] `GET /Items/Latest` – Zuletzt hinzugefügte Medien
- [x] `GET /Items/Resume` – Weiterschauen-Liste
- [x] `GET /Items/Filters` – Verfügbare Filter (Genres, Jahre, etc.)
- [x] `GET /Users/{id}/Items` – Medien mit Benutzer-spezifischem Status
- [x] `GET /Users/{id}/Items/Latest` – Zuletzt hinzugefügt pro Benutzer
- [x] `GET /Users/{id}/Items/Resume` – Weiterschauen pro Benutzer
- [x] Rekursive Ordner-Struktur (Serien → Staffeln → Episoden)
- [x] Medientyp-Erkennung (Film vs. Serie vs. Musik vs. Foto)

### Metadaten
- [x] `GET /Items/{id}/Images` – Bilder eines Mediums
- [x] `GET /Items/{id}/Images/{type}` – Bestimmtes Bild abrufen (Primary, Backdrop, etc.)
- [x] `POST /Items/{id}/Images/{type}` – Bild hochladen
- [x] Image-Proxy mit Caching und Resize
- [x] TMDb-Provider (Filme & Serien)
- [x] OMDb-Provider
- [x] Lokale NFO-Datei-Parser
- [x] Lokale Bilder erkennen (poster.jpg, fanart.jpg, etc.)
- [x] `POST /Items/{id}/Refresh` – Metadaten neu laden
- [x] Automatischer Metadaten-Abruf beim Library-Scan

## Phase 2: Streaming & Wiedergabe

### Direct Play / Direct Stream
- [x] `GET /Videos/{id}/stream` – Video-Datei direkt streamen
- [x] `GET /Audio/{id}/stream` – Audio-Datei direkt streamen
- [x] Range-Request-Support (Seeking)
- [x] `HEAD`-Requests für Medien-Streams
- [x] Content-Type-Erkennung (video/mp4, audio/flac, etc.)

### Transkodierung
- [x] `GET /Videos/{id}/master.m3u8` – HLS-Transkodierung
- [x] FFmpeg-Prozess-Management (Start, Stop, Cleanup)
- [x] Transkodierungs-Profile (Bitrate, Codec, Auflösung)
- [x] Hardware-Beschleunigung (VAAPI, NVENC, QSV)
- [x] Untertitel-Einbrennung (Burn-in)
- [x] Segment-basiertes Streaming (HLS-Segmente)
- [x] Transkodierungs-Cache-Management

### Wiedergabe-Status
- [x] `POST /Sessions/Playing` – Wiedergabe gestartet
- [x] `POST /Sessions/Playing/Progress` – Fortschritt melden
- [x] `POST /Sessions/Playing/Stopped` – Wiedergabe gestoppt
- [x] `POST /Users/{id}/PlayedItems/{itemId}` – Als gesehen markieren
- [x] `DELETE /Users/{id}/PlayedItems/{itemId}` – Als ungesehen markieren
- [x] `POST /Users/{id}/FavoriteItems/{itemId}` – Favorit setzen
- [x] Wiedergabe-Position speichern & wiederherstellen

### Untertitel
- [x] `GET /Videos/{id}/Subtitles/{index}` – Untertitel abrufen
- [x] Externe Untertitel-Dateien erkennen (.srt, .ass, .vtt)
- [x] Untertitel-Format-Konvertierung (SRT → VTT)
- [x] `GET /Items/{id}/RemoteSearch/Subtitles` – Untertitel suchen (OpenSubtitles)

## Phase 3: Session & Geräte-Management

### Sessions
- [x] `GET /Sessions` – Aktive Sessions auflisten
- [x] `POST /Sessions/Logout` – Session beenden
- [x] Session-Timeout & automatische Bereinigung
- [x] WebSocket-Verbindung für Echtzeit-Updates
- [x] `POST /Sessions/{id}/Command` – Remote-Steuerung

### Geräte
- [x] `GET /Devices` – Registrierte Geräte
- [x] `DELETE /Devices` – Gerät entfernen
- [x] Device-ID-Tracking pro Client

## Phase 4: Erweiterte Features

### Suche
- [x] `GET /Search/Hints` – Suchvorschläge
- [x] Volltextsuche über Titel, Beschreibung, Personen
- [x] Fuzzy-Search / Tippfehler-Toleranz

### Sammlungen & Playlists
- [x] `GET /Collections` – Sammlungen
- [x] `POST /Collections` – Sammlung erstellen
- [x] `GET /Playlists/{id}/Items` – Playlist-Inhalte
- [x] `POST /Playlists` – Playlist erstellen

### Personen & Genres
- [x] `GET /Persons` – Schauspieler/Regisseure
- [x] `GET /Persons/{id}` – Person-Details
- [x] `GET /Genres` – Genre-Liste
- [x] `GET /Studios` – Studio-Liste
- [x] `GET /Artists` – Künstler (Musik)

### Aktivitätslog
- [x] `GET /System/ActivityLog/Entries` – Aktivitätsprotokoll
- [x] Login-Versuche loggen
- [x] Medien-Wiedergabe loggen

### Benachrichtigungen
- [x] `GET /Notifications/{userId}` – Benachrichtigungen
- [x] Webhook-Support

## Phase 5: Netzwerk & Discovery

### DLNA
- [x] UPnP-Discovery (SSDP)
- [x] DLNA-Medienserver
- [x] DLNA-Content-Directory-Service

### Netzwerk
- [x] `GET /System/Endpoint` – Netzwerk-Endpoint-Info
- [x] Automatische Port-Erkennung
- [x] HTTPS/TLS-Support (rustls)
- [x] Reverse-Proxy-Kompatibilität (X-Forwarded-For, etc.)
- [x] IPv6-Support

## Phase 6: Setup & Administration

### Ersteinrichtung (Setup Wizard)
- [x] `GET /Startup/Configuration` – Setup-Status
- [x] `POST /Startup/Configuration` – Setup abschließen
- [x] `GET /Startup/User` – Initialen Admin-User abfragen
- [x] `POST /Startup/User` – Admin-User erstellen
- [x] `POST /Startup/Complete` – Setup abschließen
- [x] Wizard-Flow für Ersteinrichtung

### Scheduled Tasks
- [x] `GET /ScheduledTasks` – Geplante Aufgaben
- [x] `POST /ScheduledTasks/{id}/Triggers` – Task-Trigger setzen
- [x] Automatischer Library-Scan (Intervall-basiert)
- [x] Automatische Metadaten-Aktualisierung
- [x] Transkodierungs-Cache-Bereinigung

### Plugin-System
- [x] Plugin-API definieren (Trait-basiert)
- [x] Plugin-Laden (dynamisch via `libloading` oder WASM)
- [x] `GET /Plugins` – Installierte Plugins
- [x] Plugin-Konfiguration

## Phase 7: Client-Kompatibilität

### Jellyfin Web Client
- [x] Statische Web-UI ausliefern (jellyfin-web)
- [x] WebSocket-Endpoint (`/socket`)
- [x] Alle benötigten API-Responses im exakten Jellyfin-Format
- [x] CORS-Konfiguration für Web-Client

### Mobile & TV Clients
- [ ] Jellyfin Android App kompatibel
- [ ] Jellyfin iOS App kompatibel
- [ ] Jellyfin TV (Android TV, Fire TV) kompatibel
- [ ] Client-Capability-Reporting

### Drittanbieter-Clients
- [ ] Swiftfin (Apple TV) kompatibel
- [ ] Findroid (Android) kompatibel
- [ ] Sailfin kompatibel

## Phase 8: Qualität & Betrieb

### Testing
- [x] Unit-Tests für alle Repository-Funktionen
- [x] Unit-Tests für Auth-Logik
- [x] Integrationstests für API-Endpoints
- [x] End-to-End-Tests mit echtem Client

### Dokumentation
- [x] README mit Setup-Anleitung
- [x] API-Dokumentation (OpenAPI/Swagger)
- [x] Konfigurationsdokumentation
- [x] Docker-Image & Dockerfile
- [x] docker-compose.yml Beispiel

### Performance & Stabilität
- [x] Graceful Shutdown
- [x] Connection Pooling (DB)
- [x] Rate Limiting
- [x] Request-Logging (strukturiert)
- [x] Fehlerbehandlung & Error-Responses im Jellyfin-Format
- [x] Speicher-Optimierung für große Bibliotheken (>100k Items)
