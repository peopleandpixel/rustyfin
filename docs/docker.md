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
      - RUSTYFIN_SECRET=ein-sicherer-geheimer-schluessel
      - RUSTYFIN_SERVER_ID=feste-server-id
      - RUSTYFIN_API_KEY=mein-api-key
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

## Volumes

| Volume | Beschreibung |
|---|---|
| `/data` | Datenverzeichnis (Datenbank, Cache, Logs) |
| `/media` | Medienverzeichnis (read-only empfohlen) |

## Ports

| Port | Beschreibung |
|---|---|
| `8096` | HTTP-API |
