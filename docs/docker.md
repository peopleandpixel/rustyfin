<p align="center">
  <img src="rustyfin.jpeg" alt="Rustyfin Logo" width="120"/>
</p>

# Docker

## Dockerfile

Das Projekt enthält ein Multi-Stage-Dockerfile für optimale Image-Größe.

### Image bauen

```bash
docker build -t rustyfin .
```

### Container starten

```bash
docker run -d \
  --name rustyfin \
  -p 8096:8096 \
  -v /pfad/zu/medien:/media:ro \
  -v rustyfin-data:/data \
  -e RUSTYFIN_DATA=/data \
  rustyfin
```

## docker-compose

### Einfache Konfiguration

```yaml
services:
  rustyfin:
    build: .
    container_name: rustyfin
    ports:
      - "8096:8096"
    volumes:
      - /pfad/zu/medien:/media:ro
      - rustyfin-data:/data
    environment:
      - RUSTYFIN_DATA=/data
      - RUSTYFIN_BIND=0.0.0.0:8096
      - RUST_LOG=info
    restart: unless-stopped

volumes:
  rustyfin-data:
```

### Produktionskonfiguration

```yaml
services:
  rustyfin:
    build: .
    container_name: rustyfin
    ports:
      - "8096:8096"
    volumes:
      - /srv/medien/filme:/media/filme:ro
      - /srv/medien/serien:/media/serien:ro
      - /srv/medien/musik:/media/musik:ro
      - rustyfin-data:/data
    environment:
      - RUSTYFIN_DATA=/data
      - RUSTYFIN_BIND=0.0.0.0:8096
      - RUSTYFIN_SERVER_ID=feste-server-id
      # Optional, für Integrationen. Ein leerer Wert schaltet den API-Key ab.
      # Eigenen Zufallswert eintragen, z. B. die Ausgabe von: openssl rand -hex 32
      - RUSTYFIN_API_KEY=<zufallswert>
      - RUST_LOG=info
    restart: unless-stopped
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:8096/System/Info/Public"]
      interval: 30s
      timeout: 10s
      retries: 3

volumes:
  rustyfin-data:
```

### Starten mit docker-compose

```bash
# Starten
docker compose up -d

# Logs anzeigen
docker compose logs -f rustyfin

# Stoppen
docker compose down
```

### Neustart und Herunterfahren aus dem Dashboard

Rustyfin startet sich nicht selbst neu. Beide Knöpfe beenden den Prozess sauber (laufende Transkodierungen und ffmpeg-Läufe der geplanten Aufgaben werden beendet, die Datenbank geschlossen), aber mit verschiedenem Exit-Code:

| Aktion | Exit-Code | `restart: always` | `restart: unless-stopped` | `restart: on-failure` |
|---|---|---|---|---|
| **Neu starten** (`POST /System/Restart`) | `75` | startet neu | startet neu | startet neu |
| **Herunterfahren** (`POST /System/Shutdown`) | `0` | startet neu | startet neu | bleibt aus |

Mit `always` oder `unless-stopped` (wie in den Beispielen oben) wirkt Herunterfahren also wie ein Neustart; ganz aus bleibt der Container dann nur mit `docker compose stop`. Soll Herunterfahren den Server wirklich anhalten, nimm `restart: on-failure`: Dann startet nur ein Neustart (Exit-Code 75) oder ein Absturz ihn wieder. Unter systemd gilt dasselbe mit `Restart=on-failure`.

## Volumes

| Volume | Beschreibung |
|---|---|
| `/data` | Datenverzeichnis (Datenbank, Cache, Logs) |
| `/media` | Medienverzeichnis (read-only empfohlen) |

## Ports

| Port | Beschreibung |
|---|---|
| `8096` | HTTP-API |
