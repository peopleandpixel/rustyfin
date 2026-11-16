<p align="center">
  <img src="rustyfin.jpeg" alt="Rustyfin Logo" width="120"/>
</p>

# Konfiguration

Rustyfin wird über Umgebungsvariablen konfiguriert. Es sind keine Konfigurationsdateien erforderlich.

## Umgebungsvariablen

| Variable | Beschreibung | Standardwert |
|---|---|---|
| `RUSTYFIN_BIND` | Adresse und Port, auf dem der Server lauscht | `0.0.0.0:8096` |
| `RUSTYFIN_DATA` | Datenverzeichnis (Datenbank, Cache, Konfiguration, Logs) | `~/.rustyfin` |
| `RUSTYFIN_DB` | SQLite-Datenbank-URL | `sqlite://<data_dir>/rustyfin.db?mode=rwc` |
| `RUSTYFIN_SECRET` | Veraltet und ohne Wirkung — Access-Token werden serverseitig gespeichert und überstehen Neustarts | *(ungenutzt)* |
| `RUSTYFIN_SERVER_ID` | Eindeutige Server-Kennung | Stabil; in `<data_dir>/server_id` gespeichert |
| `RUSTYFIN_API_KEY` | Statischer API-Key für externe Integrationen (ein leerer Wert schaltet ihn ab) | *(deaktiviert)* |
| `RUSTYFIN_TMDB_API_KEY` | TMDb-API-Key (v3-Key oder v4-Lesezugriffstoken). Hat Vorrang vor dem Key in der Serverkonfiguration und wird dort nie gespeichert | Key aus der Konfiguration, sonst TMDb aus |
| `RUSTYFIN_TMDB_BASE_URL` | Basis-URL der TMDb-API (absolute http(s)-URL; ein ungültiger Wert verhindert den Start) | `https://api.themoviedb.org/3` |
| `RUSTYFIN_TMDB_IMAGE_BASE_URL` | Basis-URL der TMDb-Bilder (absolute http(s)-URL; ein ungültiger Wert verhindert den Start) | `https://image.tmdb.org/t/p/original` |
| `RUSTYFIN_KNOWN_PROXIES` | Reverse-Proxies, denen beim Setzen von `X-Forwarded-For` vertraut wird, kommagetrennte IPs oder CIDRs (Loopback nur, wenn eingetragen). Hinter einem Reverse Proxy diesen eintragen und ihn `X-Forwarded-For` senden lassen, siehe [Reverse Proxy](#reverse-proxy) | *(keine)* |
| `RUSTYFIN_RATE_LIMIT` | Erlaubte Anmelde- und Passwort-Reset-Versuche pro Zeitfenster, pro Client-IP (`0` deaktiviert) | `10` |
| `RUSTYFIN_RATE_WINDOW` | Zeitfenster der Ratenbegrenzung in Sekunden | `60` |
| `RUSTYFIN_IMAGE_JOBS` | Wie viele Bildaufträge (Trickplay-Bilder eines Videos, Kapitelbilder eines Eintrags, Intro-Erkennung einer Folge) gleichzeitig laufen. Erlaubt sind 1 bis zur Anzahl der CPUs, die der Prozess nutzen darf (in Docker zählt ein CPU-Limit wie `--cpus`); ein ungültiger Wert wird im Log gemeldet und durch den Standard ersetzt. Videos in Matroska oder MP4, bei denen sich das lohnt, liest Rustyfin für Trickplay-Bilder nicht ganz, sondern springt zu den nötigen Schlüsselbildern; das spart auf Netzwerkfreigaben (NFS/SMB) viel Lesezeit. Unter Unix (Linux, macOS) läuft ffmpeg dabei mit niedriger CPU-Priorität (nice 10), damit Transkodierungen Vorrang haben; unter Windows mit normaler Priorität. Trickplay-Bilder brauchen FFmpeg 7.0 oder neuer | Hälfte der CPUs, 2 bis 4 |
| `RUSTYFIN_NETWORK_READ_JOBS` | Wie viele Hintergrundaufträge, die eine ganze Mediendatei lesen (Intro-Erkennung, Trickplay-Bilder, wenn nicht zu Schlüsselbildern gesprungen wird), gleichzeitig von einer Netzwerkfreigabe lesen. Ein solcher Lesevorgang lastet die Leitung meist schon aus; weitere gleichzeitig bringen nur Suchzugriffe und Speicherbedarf. Als lokal gelten die Dateisysteme, die auch die Ordnerüberwachung als lokal kennt (ext4, xfs, btrfs, zfs usw.), und FUSE-Pools aus lokalen Platten: `fuse.mergerfs` und `fuse.shfs` (die Benutzerfreigaben von Unraid). Alles andere gilt als Netzwerkfreigabe: NFS, SMB, FUSE-Einhängungen entfernter Speicher wie rclone (`fuse.rclone`) und sshfs (`fuse.sshfs`) sowie ein Dateisystem, dessen Typ sich nicht lesen lässt. Für eine schnelle Netzwerkfreigabe kann man den Wert erhöhen; lokale Platten begrenzt nur `RUSTYFIN_IMAGE_JOBS`. Erlaubt sind 1 bis 16 | 1 |
| `RUST_LOG` | Log-Level-Filter (z.B. `debug`, `info`, `rustyfin=trace`) | `info` |
| `RUSTYFIN_LOG_HISTORY` | Anzahl der letzten Log-Zeilen, die das Live-Log der Verwaltung beim Öffnen erhält (`0` behält keine, höchstens `100000`; je Zeile bis zu 8 KiB) | `1000` |
| `TZ` | Zeitzone des Servers (z.B. `Europe/Berlin`). Die Zugriffszeiten der Benutzer gelten in dieser Zeit, siehe [Zugriffszeiten](#zugriffszeiten) | Zeitzone des Systems; in einem Container ohne `TZ` meist UTC |

## Beispielkonfigurationen

### Minimale Konfiguration

```bash
cargo run --release
```

Startet mit allen Standardwerten. Der Server lauscht auf Port 8096 und speichert Daten in `~/.rustyfin/`.

### Produktionskonfiguration

```bash
export RUSTYFIN_BIND=127.0.0.1:8096
export RUSTYFIN_DATA=/srv/rustyfin
export RUSTYFIN_SERVER_ID=feste-server-id-hier
# Eigenen Zufallswert eintragen, z. B. die Ausgabe von: openssl rand -hex 32
export RUSTYFIN_API_KEY='<zufallswert>'
export RUST_LOG=info

./target/release/rustyfin
```

### Entwicklungskonfiguration

```bash
RUST_LOG=rustyfin=debug cargo run
```

## Datenverzeichnis

Das Datenverzeichnis (`RUSTYFIN_DATA`) enthält:

```
~/.rustyfin/
├── rustyfin.db          # SQLite-Datenbank
├── cache/               # Bild- und Metadaten-Cache
│   └── images/          # Gecachte und skalierte Bilder
└── logs/                # Log-Dateien
```

## Netzwerk

### Port-Konfiguration

Der Standardport ist `8096` (identisch mit Jellyfin). Zum Ändern:

```bash
RUSTYFIN_BIND=0.0.0.0:9090 ./target/release/rustyfin
```

### Reverse Proxy

Rustyfin unterstützt den Betrieb hinter einem Reverse Proxy. Die folgenden Header werden ausgewertet:

- `X-Forwarded-For`
- `X-Forwarded-Proto`
- `X-Forwarded-Host`

`X-Forwarded-For` wird nur ausgewertet, wenn die Verbindung von einem in `RUSTYFIN_KNOWN_PROXIES` eingetragenen Proxy stammt; das gilt auch für Loopback (`127.0.0.1`, `::1`). Rustyfin liest alle `X-Forwarded-For`-Zeilen von rechts nach links und nimmt die erste Adresse, die kein eingetragener Proxy ist; `ip`, `ip:port`, `[v6]` und `[v6]:port` werden erkannt. Ist ein Eintrag unlesbar oder sind alle Einträge eingetragene Proxies, zählt die Anfrage als vom Proxy kommend: Sie ist weder lokal noch von der Ratenbegrenzung ausgenommen. Dasselbe gilt für eine Gegenstelle, die nicht eingetragen ist, aber `X-Forwarded-For`, `X-Real-IP` oder `Forwarded` sendet; Rustyfin wertet diese Header dann nicht aus, und alle Clients hinter ihr teilen sich ein Budget. IPv6-Clients teilen sich ein Budget pro /56-Netz.

> ⚠️ **Hinter einem Reverse Proxy `RUSTYFIN_KNOWN_PROXIES` setzen und den Proxy `X-Forwarded-For` senden lassen.** Für das Beispiel unten: `RUSTYFIN_KNOWN_PROXIES=127.0.0.1`. Ein nicht eingetragener Proxy, der `X-Forwarded-For`, `X-Real-IP` oder `Forwarded` sendet, ist weder lokal noch von der Ratenbegrenzung ausgenommen, und alle seine Clients teilen sich ein Budget: Über ihn kann niemand einen Passwort-Reset starten, und die Anmeldeversuche aller Clients zählen gemeinsam. Sendet ein Proxy keinen dieser Header, sieht er wie ein direkter Client aus, und das kann der Server nicht erkennen. Läuft er auf demselben Host, gilt dann jeder Client als Loopback: von der Ratenbegrenzung ausgenommen und berechtigt, einen Passwort-Reset zu starten.

#### Nginx-Beispiel

```nginx
server {
    listen 443 ssl;
    server_name media.example.com;

    ssl_certificate /etc/ssl/certs/media.pem;
    ssl_certificate_key /etc/ssl/private/media.key;

    location / {
        proxy_pass http://127.0.0.1:8096;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;

        # WebSocket-Support
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
    }
}
```

## Authentifizierung

### Zugriffstoken

Rustyfin gibt pro Gerät ein undurchsichtiges Zugriffstoken aus und speichert es serverseitig in der Datenbank. Die Token überstehen einen Neustart und bleiben gültig, bis sich das Gerät abmeldet, das Passwort des Benutzers geändert wird oder das Konto deaktiviert wird.

> ℹ️ **`RUSTYFIN_SECRET` wird nicht mehr benötigt.** Die Variable wird ignoriert; Sessions bleiben über Neustarts hinweg erhalten.

### Passwort zurücksetzen (PIN)

„Passwort vergessen“ erzeugt einen PIN und schreibt ihn in die Datei `passwordreset-<hash>.json` im Datenverzeichnis; die Antwort nennt nur den Dateipfad, nie den PIN. Die Datei wird nur geschrieben, wenn der Benutzer existiert und die Anfrage aus dem lokalen Netz kommt (Loopback, private IPv4-Bereiche, Link-Local, IPv6 ULA und Link-Local; die Client-Adresse wird wie bei der Ratenbegrenzung über `RUSTYFIN_KNOWN_PROXIES` bestimmt). Der PIN gilt 30 Minuten und ist nach 5 falschen Eingaben ungültig. Pro Benutzer gibt es höchstens einen offenen Reset; eine neue Anfrage erzeugt erst nach dessen Ablauf einen neuen PIN. Nach dem Einlösen ist der PIN das neue Passwort, und alle Sitzungen des Benutzers werden beendet.

> ℹ️ **Docker:** Im Bridge-Netz kommen Verbindungen von außen oft von der Gateway-Adresse (z. B. `172.17.0.1`). Diese Adresse ist privat und zählt als lokales Netz, daher kann dann jeder, der den Port erreicht, einen Reset starten (der PIN landet trotzdem nur in der Datei).

### API-Key

Ein statischer API-Key kann über `RUSTYFIN_API_KEY` gesetzt werden, zum Beispiel ein mit `openssl rand -hex 32` erzeugter Zufallswert. Ein leerer Wert schaltet den API-Key ab. Der Key kann für externe Integrationen verwendet werden:

```bash
curl -H "Authorization: MediaBrowser Token=<api-key>" \
  http://localhost:8096/System/Info
```

### Quick Connect

Quick Connect ermöglicht die Anmeldung ohne Passwort über einen temporären Code. Diese Funktion ist standardmäßig aktiviert.

### Zugriffszeiten

Ein Administrator kann einem Benutzer Zugriffszeiten geben (Verwaltung → Benutzer → Bearbeiten → Jugendschutz). Außerhalb dieser Zeiten kann sich ein Benutzer, der kein Administrator ist, nicht anmelden, und jede Anfrage wird mit 403 abgelehnt, wie bei Jellyfin. Die Zeiten gelten in der Ortszeit des Servers: Setze `TZ` (z.B. `TZ=Europe/Berlin`), sonst rechnet ein Container meist in UTC. Das Docker-Image enthält die Zeitzonendaten (`tzdata`).

## Logging

Die Log-Ausgabe wird über die Umgebungsvariable `RUST_LOG` gesteuert:

```bash
# Nur Fehler
RUST_LOG=error

# Info-Level (Standard)
RUST_LOG=info

# Debug für Rustyfin, Info für alles andere
RUST_LOG=info,rustyfin=debug

# Vollständiges Tracing
RUST_LOG=rustyfin=trace
```
