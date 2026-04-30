<p align="center">
  <img src="rustyfin.jpeg" alt="Rustyfin Logo" width="120"/>
</p>

# API-Dokumentation

Rustyfin implementiert die Jellyfin-API. Alle Endpoints verwenden JSON als Datenformat.

## Authentifizierung

Die meisten Endpoints erfordern Authentifizierung über den `Authorization`-Header:

```
Authorization: MediaBrowser Token=<jwt-token-oder-api-key>
```

Alternativ kann der Token als Query-Parameter übergeben werden:

```
GET /System/Info?api_key=<token>
```

---

## Öffentliche Endpoints (ohne Authentifizierung)

### System

| Methode | Endpoint | Beschreibung |
|---|---|---|
| `GET` | `/System/Info/Public` | Öffentliche Server-Informationen |
| `GET` | `/System/Endpoint` | Netzwerk-Endpoint-Info |
| `GET` | `/web/config.json` | Web-Client-Konfiguration |

### Authentifizierung

| Methode | Endpoint | Beschreibung |
|---|---|---|
| `POST` | `/Users/AuthenticateByName` | Login mit Benutzername und Passwort |

### Quick Connect

| Methode | Endpoint | Beschreibung |
|---|---|---|
| `GET` | `/QuickConnect/Enabled` | Quick Connect Status prüfen |
| `POST` | `/QuickConnect/Initiate` | Quick Connect starten |
| `GET` | `/QuickConnect/Connect` | Quick Connect Status abfragen |

### Setup (Ersteinrichtung)

| Methode | Endpoint | Beschreibung |
|---|---|---|
| `GET` | `/Startup/Configuration` | Setup-Status abfragen |
| `POST` | `/Startup/Configuration` | Setup-Konfiguration setzen |
| `GET` | `/Startup/User` | Initialen Admin-User abfragen |
| `POST` | `/Startup/User` | Admin-User erstellen |
| `POST` | `/Startup/Complete` | Setup abschließen |

---

## Geschützte Endpoints (Authentifizierung erforderlich)

### System

| Methode | Endpoint | Beschreibung |
|---|---|---|
| `GET` | `/System/Info` | Vollständige Server-Informationen |
| `POST` | `/System/Restart` | Server neustarten |
| `POST` | `/System/Shutdown` | Server herunterfahren |
| `GET` | `/System/Logs` | Server-Logs abrufen |

### Benutzer

| Methode | Endpoint | Beschreibung |
|---|---|---|
| `GET` | `/Users` | Alle Benutzer auflisten |
| `GET` | `/Users/{id}` | Benutzer-Details |
| `POST` | `/Users/New` | Neuen Benutzer erstellen |
| `POST` | `/Users/{id}` | Benutzer aktualisieren |
| `DELETE` | `/Users/{id}` | Benutzer löschen |
| `POST` | `/Users/{id}/Password` | Passwort ändern |
| `POST` | `/Users/{id}/Policy` | Benutzerrichtlinie setzen |
| `POST` | `/Users/{id}/Configuration` | Benutzereinstellungen setzen |

### Bibliothek

| Methode | Endpoint | Beschreibung |
|---|---|---|
| `GET` | `/Library/VirtualFolders` | Alle Bibliotheken auflisten |
| `POST` | `/Library/VirtualFolders` | Neue Bibliothek erstellen |
| `DELETE` | `/Library/VirtualFolders` | Bibliothek löschen |
| `POST` | `/Library/Refresh` | Alle Bibliotheken neu scannen |
| `GET` | `/Library/MediaFolders` | Medienordner auflisten |

### Items (Medien)

| Methode | Endpoint | Beschreibung |
|---|---|---|
| `GET` | `/Items` | Medien durchsuchen (mit Filter, Sortierung, Paginierung) |
| `GET` | `/Items/{id}` | Item-Details |
| `GET` | `/Users/{userId}/Items` | Items für Benutzer (mit Wiedergabestatus) |
| `GET` | `/Users/{userId}/Items/{id}` | Item-Details für Benutzer |
| `GET` | `/Items/{id}/Similar` | Ähnliche Items |
| `GET` | `/Shows/{id}/Seasons` | Staffeln einer Serie |
| `GET` | `/Shows/{id}/Episodes` | Episoden einer Serie/Staffel |
| `GET` | `/Items/Latest` | Zuletzt hinzugefügte Items |
| `GET` | `/Items/Resume` | Weiterschauen |

### Bilder

| Methode | Endpoint | Beschreibung |
|---|---|---|
| `GET` | `/Items/{id}/Images/{type}` | Bild eines Items abrufen |
| `GET` | `/Items/{id}/Images/{type}/{index}` | Bild mit Index abrufen |

Unterstützte Bildtypen: `Primary`, `Backdrop`, `Banner`, `Thumb`, `Logo`

Query-Parameter für Bildskalierung:
- `maxWidth` – Maximale Breite
- `maxHeight` – Maximale Höhe
- `quality` – JPEG-Qualität (1-100)

### Streaming

| Methode | Endpoint | Beschreibung |
|---|---|---|
| `GET` | `/Videos/{id}/stream` | Video-Stream (Direct Play) |
| `GET` | `/Audio/{id}/stream` | Audio-Stream (Direct Play) |
| `GET` | `/Videos/{id}/master.m3u8` | HLS Master-Playlist |
| `GET` | `/Videos/{id}/main.m3u8` | HLS Stream-Playlist |

### Wiedergabe

| Methode | Endpoint | Beschreibung |
|---|---|---|
| `POST` | `/Sessions/Playing` | Wiedergabe gestartet melden |
| `POST` | `/Sessions/Playing/Progress` | Wiedergabefortschritt melden |
| `POST` | `/Sessions/Playing/Stopped` | Wiedergabe beendet melden |
| `POST` | `/Users/{userId}/PlayedItems/{id}` | Als gesehen markieren |
| `DELETE` | `/Users/{userId}/PlayedItems/{id}` | Als ungesehen markieren |
| `POST` | `/Users/{userId}/FavoriteItems/{id}` | Als Favorit markieren |
| `DELETE` | `/Users/{userId}/FavoriteItems/{id}` | Favorit entfernen |

### Untertitel

| Methode | Endpoint | Beschreibung |
|---|---|---|
| `GET` | `/Videos/{id}/Subtitles/{index}/Stream.{format}` | Untertitel abrufen |

### Suche

| Methode | Endpoint | Beschreibung |
|---|---|---|
| `GET` | `/Search/Hints` | Medien durchsuchen |

### Sammlungen

| Methode | Endpoint | Beschreibung |
|---|---|---|
| `POST` | `/Collections` | Sammlung erstellen |
| `POST` | `/Collections/{id}/Items` | Items zur Sammlung hinzufügen |
| `DELETE` | `/Collections/{id}/Items` | Items aus Sammlung entfernen |

### Personen

| Methode | Endpoint | Beschreibung |
|---|---|---|
| `GET` | `/Persons` | Personen auflisten |
| `GET` | `/Persons/{name}` | Person-Details |

### Sessions

| Methode | Endpoint | Beschreibung |
|---|---|---|
| `GET` | `/Sessions` | Aktive Sessions auflisten |
| `POST` | `/Sessions/Capabilities/Full` | Client-Capabilities melden |

### Aktivität

| Methode | Endpoint | Beschreibung |
|---|---|---|
| `GET` | `/System/ActivityLog/Entries` | Aktivitätslog abrufen |

### Geplante Aufgaben

| Methode | Endpoint | Beschreibung |
|---|---|---|
| `GET` | `/ScheduledTasks` | Geplante Aufgaben auflisten |
| `POST` | `/ScheduledTasks/{id}/Triggers` | Task-Trigger setzen |

### Plugins

| Methode | Endpoint | Beschreibung |
|---|---|---|
| `GET` | `/Plugins` | Installierte Plugins auflisten |

### DLNA

| Methode | Endpoint | Beschreibung |
|---|---|---|
| `GET` | `/Dlna/ProfileInfos` | DLNA-Profile auflisten |

---

## Fehlerbehandlung

Alle Fehler werden im Jellyfin-kompatiblen Format zurückgegeben:

```json
{
  "code": 404,
  "message": "Item not found"
}
```

### HTTP-Statuscodes

| Code | Bedeutung |
|---|---|
| `200` | Erfolg |
| `204` | Erfolg (ohne Inhalt) |
| `400` | Ungültige Anfrage |
| `401` | Nicht authentifiziert |
| `403` | Keine Berechtigung |
| `404` | Nicht gefunden |
| `500` | Interner Serverfehler |
