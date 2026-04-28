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
- [ ] `GET /Videos/{id}/master.m3u8` – HLS-Transkodierung
- [ ] FFmpeg-Prozess-Management (Start, Stop, Cleanup)
- [ ] Transkodierungs-Profile (Bitrate, Codec, Auflösung)
- [ ] Hardware-Beschleunigung (VAAPI, NVENC, QSV)
- [ ] Untertitel-Einbrennung (Burn-in)
- [ ] Segment-basiertes Streaming (HLS-Segmente)
- [ ] Transkodierungs-Cache-Management

### Wiedergabe-Status
- [ ] `POST /Sessions/Playing` – Wiedergabe gestartet
- [ ] `POST /Sessions/Playing/Progress` – Fortschritt melden
- [ ] `POST /Sessions/Playing/Stopped` – Wiedergabe gestoppt
- [ ] `POST /Users/{id}/PlayedItems/{itemId}` – Als gesehen markieren
- [ ] `DELETE /Users/{id}/PlayedItems/{itemId}` – Als ungesehen markieren
- [ ] `POST /Users/{id}/FavoriteItems/{itemId}` – Favorit setzen
- [ ] Wiedergabe-Position speichern & wiederherstellen

### Untertitel
- [ ] `GET /Videos/{id}/Subtitles/{index}` – Untertitel abrufen
- [ ] Externe Untertitel-Dateien erkennen (.srt, .ass, .vtt)
- [ ] Untertitel-Format-Konvertierung (SRT → VTT)
- [ ] `GET /Items/{id}/RemoteSearch/Subtitles` – Untertitel suchen (OpenSubtitles)

## Phase 3: Session & Geräte-Management

### Sessions
- [ ] `GET /Sessions` – Aktive Sessions auflisten
- [ ] `POST /Sessions/Logout` – Session beenden
- [ ] Session-Timeout & automatische Bereinigung
- [ ] WebSocket-Verbindung für Echtzeit-Updates
- [ ] `POST /Sessions/{id}/Command` – Remote-Steuerung

### Geräte
- [ ] `GET /Devices` – Registrierte Geräte
- [ ] `DELETE /Devices` – Gerät entfernen
- [ ] Device-ID-Tracking pro Client

## Phase 4: Erweiterte Features

### Suche
- [ ] `GET /Search/Hints` – Suchvorschläge
- [ ] Volltextsuche über Titel, Beschreibung, Personen
- [ ] Fuzzy-Search / Tippfehler-Toleranz

### Sammlungen & Playlists
- [ ] `GET /Collections` – Sammlungen
- [ ] `POST /Collections` – Sammlung erstellen
- [ ] `GET /Playlists/{id}/Items` – Playlist-Inhalte
- [ ] `POST /Playlists` – Playlist erstellen

### Personen & Genres
- [ ] `GET /Persons` – Schauspieler/Regisseure
- [ ] `GET /Persons/{id}` – Person-Details
- [ ] `GET /Genres` – Genre-Liste
- [ ] `GET /Studios` – Studio-Liste
- [ ] `GET /Artists` – Künstler (Musik)

### Aktivitätslog
- [ ] `GET /System/ActivityLog/Entries` – Aktivitätsprotokoll
- [ ] Login-Versuche loggen
- [ ] Medien-Wiedergabe loggen

### Benachrichtigungen
- [ ] `GET /Notifications/{userId}` – Benachrichtigungen
- [ ] Webhook-Support

## Phase 5: Netzwerk & Discovery

### DLNA
- [ ] UPnP-Discovery (SSDP)
- [ ] DLNA-Medienserver
- [ ] DLNA-Content-Directory-Service

### Netzwerk
- [ ] `GET /System/Endpoint` – Netzwerk-Endpoint-Info
- [ ] Automatische Port-Erkennung
- [ ] HTTPS/TLS-Support (rustls)
- [ ] Reverse-Proxy-Kompatibilität (X-Forwarded-For, etc.)
- [ ] IPv6-Support

## Phase 6: Setup & Administration

### Ersteinrichtung (Setup Wizard)
- [ ] `GET /Startup/Configuration` – Setup-Status
- [ ] `POST /Startup/Configuration` – Setup abschließen
- [ ] `GET /Startup/User` – Initialen Admin-User abfragen
- [ ] `POST /Startup/User` – Admin-User erstellen
- [ ] `POST /Startup/Complete` – Setup abschließen
- [ ] Wizard-Flow für Ersteinrichtung

### Scheduled Tasks
- [ ] `GET /ScheduledTasks` – Geplante Aufgaben
- [ ] `POST /ScheduledTasks/{id}/Triggers` – Task-Trigger setzen
- [ ] Automatischer Library-Scan (Intervall-basiert)
- [ ] Automatische Metadaten-Aktualisierung
- [ ] Transkodierungs-Cache-Bereinigung

### Plugin-System
- [ ] Plugin-API definieren (Trait-basiert)
- [ ] Plugin-Laden (dynamisch via `libloading` oder WASM)
- [ ] `GET /Plugins` – Installierte Plugins
- [ ] Plugin-Konfiguration

## Phase 7: Client-Kompatibilität

### Jellyfin Web Client
- [ ] Statische Web-UI ausliefern (jellyfin-web)
- [ ] WebSocket-Endpoint (`/socket`)
- [ ] Alle benötigten API-Responses im exakten Jellyfin-Format
- [ ] CORS-Konfiguration für Web-Client

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
- [ ] Unit-Tests für alle Repository-Funktionen
- [ ] Unit-Tests für Auth-Logik
- [ ] Integrationstests für API-Endpoints
- [ ] End-to-End-Tests mit echtem Client

### Dokumentation
- [ ] README mit Setup-Anleitung
- [ ] API-Dokumentation (OpenAPI/Swagger)
- [ ] Konfigurationsdokumentation
- [ ] Docker-Image & Dockerfile
- [ ] docker-compose.yml Beispiel

### Performance & Stabilität
- [ ] Graceful Shutdown
- [ ] Connection Pooling (DB)
- [ ] Rate Limiting
- [ ] Request-Logging (strukturiert)
- [ ] Fehlerbehandlung & Error-Responses im Jellyfin-Format
- [ ] Speicher-Optimierung für große Bibliotheken (>100k Items)
