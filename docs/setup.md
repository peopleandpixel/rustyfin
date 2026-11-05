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

### Administratorkonto

Es wird kein Standardbenutzer angelegt. Das erste Administratorkonto (der Server-Besitzer) erstellen Sie im Setup-Wizard.

### Setup-Wizard

Rustyfin bietet einen Setup-Wizard für die Ersteinrichtung über die API:

1. **Status prüfen:** `GET /System/Info/Public`. `StartupWizardCompleted` ist `false`, solange der Server keine Benutzer hat.
2. **Konfiguration setzen:** `POST /Startup/Configuration`
3. **Server-Besitzer anlegen:** `POST /Startup/User`. Das ist nur einmal möglich.
4. **Anmelden:** `POST /Users/AuthenticateByName`
5. **Setup abschließen:** `POST /Startup/Complete`, mit dem Token aus Schritt 4.

Solange der Server keine Benutzer hat, benötigen die `/Startup/*`-Endpunkte keine Anmeldung; danach können nur Administratoren sie nutzen. `POST /Startup/User` ändert nie ein bestehendes Konto.

Hat ein Server nach einem Update von einer älteren Version Benutzer, aber keinen Server-Besitzer, wird beim ersten Start der zuerst angelegte aktivierte Administrator zum Server-Besitzer, auch wenn die Einrichtung nie abgeschlossen wurde. Der Server vermerkt die Einrichtung dann in `config.json` als abgeschlossen. Ältere Versionen legten beim Start das Konto `admin` mit dem Passwort `admin` an; wird es so zum Server-Besitzer, nennt der Server es bei jedem Start in einer Warnung, bis Sie das Passwort ändern.

Ist kein Administrator aktiviert, bleibt der Setup-Wizard geschlossen, und der Server protokolliert einen Fehler. Um einen Administrator festzulegen, starten Sie den Server mit `RUSTYFIN_API_KEY`, suchen die Id eines Benutzers mit `GET /Users` und senden `POST /Users/{id}/Policy` mit dem Body `{"IsAdministrator": true}`, jeweils mit dem Key im Header `X-Emby-Token`. Nach einem Neustart wird dieser Administrator zum Server-Besitzer. Ein deaktiviertes Konto lässt sich nur in der Datenbank wieder aktivieren (`users.is_disabled = 0`).

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
