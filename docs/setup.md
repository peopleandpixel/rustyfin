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

#### Import aus Jellyfin im Setup-Wizard

Der Wizard im Web-Client fragt nach der Sprache, ob Sie aus Jellyfin importieren möchten. Mit „Ja“:

1. **Jellyfin-Datenordner:** der Ordner mit `data/jellyfin.db` (im offiziellen Image `/config`). Der Server liest ihn mit `POST /Startup/JellyfinImport/Preview`.
2. **Ihr Jellyfin-Benutzer:** Sein Wiedergabeverlauf geht in Ihr neues Konto.
3. **Bibliotheken:** Jellyfins Bibliotheken mit Name, Typ und Pfaden. Liegen die Medien auf diesem Server woanders (etwa `/mnt/media` statt `/media`), korrigieren Sie den Pfad; jeder Pfad wird geprüft. Bibliotheken eines Typs, den Rustyfin nicht kennt (etwa Bücher), werden nicht angelegt.
4. **Weitere Benutzer:** Jeder ausgewählte Benutzer bekommt ein Konto und einen Einmal-Code für die Kontoeinrichtung.
5. **Konto:** Der Name ist mit dem Jellyfin-Benutzer vorbelegt. Jellyfin-Passwörter werden nicht übernommen; wählen Sie ein neues.

Mit „Nein“ legen Sie im Bibliotheksschritt beliebig viele Bibliotheken an oder keine, jede mit Name, Typ und einem oder mehreren Ordnern; jeder Ordner wird bei „Weiter“ geprüft. Beim Abschließen legt der Wizard sie an. Schlägt eine fehl, bleiben die übrigen angelegt, und ein erneutes Abschließen legt nur die fehlenden an.

Jede ausgewählte Bibliothek braucht einen Pfad auf diesem Server. Jellyfins Heimvideos werden zu einer gemischten Bibliothek, da Rustyfins Foto-Bibliotheken nur Bilder einlesen.

Beim Abschließen legt der Wizard den Server-Besitzer an, meldet sich an und stellt den Import mit `POST /JellyfinImport/Queue` ein. Schlägt das fehl, nachdem das Konto angelegt wurde, zeigt der Wizard den Fehler: Sie können die Import-Schritte korrigieren und es erneut versuchen, oder die Einrichtung mit „Ohne Import abschließen“ beenden und den Import später im Dashboard ausführen. Der Server legt die Bibliotheken an, scannt sie und importiert danach im Hintergrund. Der Wizard öffnet die Import-Seite im Dashboard, die den Fortschritt, das Ergebnis und die Einmal-Codes zeigt; die Codes erscheinen nur einmal und lassen sich dort für noch nicht eingerichtete Benutzer neu ausstellen.

Der eingestellte Import steht bis zu seinem Ende in `jellyfin-import-queue.json` im Datenordner. Wird der Server während der Scans neu gestartet, setzt er den Import fort; wird er während des Imports selbst neu gestartet, meldet die Import-Seite den Import als fehlgeschlagen, statt ihn ein zweites Mal auszuführen. Solange ein Import eingestellt ist oder läuft, beantwortet `POST /JellyfinImport/Queue` weitere Aufträge mit 409. Ein Abbrechen gibt es nicht: Hängt ein Scan, hält der eingestellte Import den Import-Platz auch über Neustarts hinweg, bis der Scan endet oder `jellyfin-import-queue.json` bei gestopptem Server gelöscht wird.

`POST /Startup/JellyfinImport/Preview` ist nur erreichbar, solange der Server keine Benutzer hat (danach 403, auch für Administratoren; dann gilt `POST /JellyfinImport/Preview`). Es läuft immer nur ein solcher Lesevorgang; ein gleichzeitiger zweiter bekommt 409. Es nennt nur Benutzernamen, Administrator-Flags, die Zahl abgespielter Elemente, Bibliotheksnamen, -typen und -pfade und die Zahl der Elemente, nie Passwort-Hashes, Tokens oder API-Keys, und schreibt nichts.

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
