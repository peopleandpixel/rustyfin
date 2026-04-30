<p align="center">
  <img src="rustyfin.jpeg" alt="Rustyfin Logo" width="120"/>
</p>

# Setup-Anleitung

## Voraussetzungen

- **Rust** 1.85+ (Edition 2024)
- **SQLite** (wird über sqlx gebündelt mitgeliefert)
- *(Optional)* TMDb API-Key für Metadaten-Abruf
- *(Optional)* OMDb API-Key für Metadaten-Abruf

## Installation aus Quellcode

### 1. Repository klonen

```bash
git clone https://github.com/<your-username>/rustyfin.git
cd rustyfin
```

### 2. Release-Build erstellen

```bash
cargo build --release
```

Die fertige Binary befindet sich unter `target/release/rustyfin`.

### 3. Server starten

```bash
cargo run --release
```

Oder direkt die Binary ausführen:

```bash
./target/release/rustyfin
```

## Ersteinrichtung

Beim ersten Start erstellt Rustyfin automatisch:

1. Das Datenverzeichnis (Standard: `~/.rustyfin/`)
2. Die SQLite-Datenbank
3. Einen Standard-Admin-Benutzer

### Standard-Zugangsdaten

| Feld | Wert |
|---|---|
| Benutzername | `admin` |
| Passwort | `admin` |

> ⚠️ **Ändern Sie das Standard-Passwort sofort nach dem ersten Login!**

### Setup-Wizard

Rustyfin bietet einen Setup-Wizard für die Ersteinrichtung über die API:

1. **Status prüfen:** `GET /Startup/Configuration`
2. **Admin-User erstellen:** `POST /Startup/User`
3. **Konfiguration setzen:** `POST /Startup/Configuration`
4. **Setup abschließen:** `POST /Startup/Complete`

## Medienbibliothek einrichten

Nach der Ersteinrichtung können Medienbibliotheken über die API angelegt werden:

```bash
# Bibliothek erstellen (Authentifizierung erforderlich)
curl -X POST http://localhost:8096/Library/VirtualFolders \
  -H "Authorization: MediaBrowser Token=<your-token>" \
  -H "Content-Type: application/json" \
  -d '{
    "Name": "Filme",
    "CollectionType": "movies",
    "Paths": ["/pfad/zu/filmen"]
  }'
```

### Unterstützte Bibliothekstypen

| Typ | CollectionType |
|---|---|
| Filme | `movies` |
| Serien | `tvshows` |
| Musik | `music` |

## Nächste Schritte

- [Konfiguration](configuration.md) – Alle Umgebungsvariablen und Optionen
- [API-Dokumentation](api.md) – Vollständige API-Referenz
- [Docker](docker.md) – Container-basierte Installation
