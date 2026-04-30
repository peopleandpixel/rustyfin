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
| `RUSTYFIN_SECRET` | JWT-Signierungsschlüssel | Zufällige UUID (bei jedem Start neu generiert) |
| `RUSTYFIN_SERVER_ID` | Eindeutige Server-Kennung | Zufällige UUID (bei jedem Start neu generiert) |
| `RUSTYFIN_API_KEY` | Statischer API-Key für externe Integrationen | *(deaktiviert)* |
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
export RUSTYFIN_SECRET=ein-sicherer-geheimer-schluessel
export RUSTYFIN_SERVER_ID=feste-server-id-hier
export RUSTYFIN_API_KEY=mein-api-key
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

### JWT

Rustyfin verwendet JWT-Token zur Authentifizierung. Der Signierungsschlüssel wird über `RUSTYFIN_SECRET` gesetzt. 

> ⚠️ **In Produktionsumgebungen immer einen festen `RUSTYFIN_SECRET` setzen!** Andernfalls werden alle Sessions bei einem Neustart ungültig.

### API-Key

Ein statischer API-Key kann über `RUSTYFIN_API_KEY` gesetzt werden. Dieser kann für externe Integrationen verwendet werden:

```bash
curl -H "Authorization: MediaBrowser Token=mein-api-key" \
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
