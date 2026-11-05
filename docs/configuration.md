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
| `RUSTYFIN_KNOWN_PROXIES` | Reverse-Proxies, denen beim Setzen von `X-Forwarded-For` vertraut wird, kommagetrennte IPs oder CIDRs (Loopback nur, wenn eingetragen). Hinter einem Reverse Proxy diesen eintragen und ihn `X-Forwarded-For` senden lassen, siehe [Reverse Proxy](#reverse-proxy) | *(keine)* |
| `RUSTYFIN_RATE_LIMIT` | Erlaubte Anmelde- und Passwort-Reset-Versuche pro Zeitfenster, pro Client-IP (`0` deaktiviert) | `10` |
| `RUSTYFIN_RATE_WINDOW` | Zeitfenster der Ratenbegrenzung in Sekunden | `60` |
| `RUST_LOG` | Log-Level-Filter (z.B. `debug`, `info`, `rustyfin=trace`) | `info` |

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

`X-Forwarded-For` wird nur ausgewertet, wenn die Verbindung von einem in `RUSTYFIN_KNOWN_PROXIES` eingetragenen Proxy stammt; das gilt auch für Loopback (`127.0.0.1`, `::1`). Rustyfin liest alle `X-Forwarded-For`-Zeilen von rechts nach links und nimmt die erste Adresse, die kein eingetragener Proxy ist; `ip`, `ip:port`, `[v6]` und `[v6]:port` werden erkannt. Ist ein Eintrag unlesbar oder sind alle Einträge eingetragene Proxies, zählt die Anfrage als vom Proxy kommend: Sie ist weder lokal noch von der Ratenbegrenzung ausgenommen. Dasselbe gilt für eine Gegenstelle, die nicht eingetragen ist, aber `X-Forwarded-For`, `X-Real-IP` oder `Forwarded` sendet; Rustyfin wertet diese Header dann nicht aus, und alle Clients hinter ihr teilen sich ein Budget. IPv6-Clients teilen sich ein Budget pro /64-Netz.

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
