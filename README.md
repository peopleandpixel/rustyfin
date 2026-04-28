# Rustyfin

A lightweight, Jellyfin-compatible media server written in Rust.

Rustyfin aims to provide a fast, resource-efficient alternative to [Jellyfin](https://jellyfin.org/) by implementing its API, allowing you to use existing Jellyfin clients to browse and stream your media library.

> **⚠️ Early Development** — This project is under active development. Many features are already working, but not all Jellyfin API endpoints are implemented yet. See [Roadmap](#roadmap) for details.

## Features

- **Jellyfin-compatible API** — Works with existing Jellyfin clients
- **Library management** — Scan and organize movies, series, and music
- **Metadata providers** — TMDb, OMDb, and local NFO file parsing
- **Image handling** — Proxy with caching and on-the-fly resizing
- **Direct Play / Direct Stream** — Stream video and audio files with range-request (seeking) support
- **Authentication** — JWT-based auth, API key support, and Quick Connect
- **User management** — Multiple users with individual policies and preferences
- **SQLite database** — Zero-configuration, embedded database
- **Low resource usage** — Built with async Rust (Tokio + Axum)

## Requirements

- **Rust** 1.85+ (edition 2024)
- **SQLite** (bundled via sqlx)
- *(Optional)* TMDb / OMDb API keys for metadata fetching

## Getting Started

### Build from Source

```bash
git clone https://github.com/<your-username>/rustyfin.git
cd rustyfin
cargo build --release
```

The binary will be at `target/release/rustyfin`.

### Run

```bash
cargo run --release
```

By default, Rustyfin listens on `0.0.0.0:8096` and stores data in `~/.rustyfin/`.

On first start, a default admin user is created:
- **Username:** `admin`
- **Password:** `admin`

> **Change the default password immediately after first login.**

## Configuration

Rustyfin is configured via environment variables:

| Variable | Description | Default |
|---|---|---|
| `RUSTYFIN_BIND` | Address and port to listen on | `0.0.0.0:8096` |
| `RUSTYFIN_DATA` | Data directory (database, cache, config, logs) | `~/.rustyfin` |
| `RUSTYFIN_DB` | SQLite database URL | `sqlite://<data_dir>/rustyfin.db?mode=rwc` |
| `RUSTYFIN_SECRET` | JWT signing secret | Random UUID (generated on each start) |
| `RUSTYFIN_SERVER_ID` | Unique server identifier | Random UUID (generated on each start) |
| `RUSTYFIN_API_KEY` | Static API key for external integrations | *(disabled)* |
| `RUST_LOG` | Log level filter (e.g. `debug`, `info`, `rustyfin=trace`) | `info` |

### Example

```bash
RUSTYFIN_BIND=127.0.0.1:8096 \
RUSTYFIN_DATA=/srv/rustyfin \
RUSTYFIN_SECRET=my-super-secret \
RUSTYFIN_API_KEY=my-api-key \
cargo run --release
```

## Project Structure

Rustyfin is organized as a Cargo workspace with the following crates:

| Crate | Description |
|---|---|
| `rustyfin-server` | Application entry point and server bootstrap |
| `rustyfin-api` | Axum routes and HTTP handlers |
| `rustyfin-auth` | Authentication (Argon2 password hashing, JWT, Quick Connect) |
| `rustyfin-core` | Core business logic (library scanning, metadata fetching, image processing) |
| `rustyfin-db` | Database layer (SQLite via sqlx, migrations, repositories) |
| `rustyfin-model` | Shared data models and DTOs |
| `rustyfin-cli` | Command-line interface utilities |

## Roadmap

### ✅ Implemented
- System & server info endpoints
- User authentication & management (JWT, API key, Quick Connect)
- Library management (create, scan, delete)
- Media browsing (items, filtering, sorting, pagination)
- Metadata providers (TMDb, OMDb, NFO)
- Image handling with caching and resizing
- Direct Play video & audio streaming with seeking

### 🚧 Planned
- HLS transcoding (FFmpeg)
- Playback status tracking (watched, resume, favorites)
- Subtitle support (SRT, ASS, VTT, OpenSubtitles)
- Session & device management
- Search (full-text, fuzzy)
- Collections & playlists
- Scheduled tasks (auto-scan, cache cleanup)
- Plugin system
- Full Jellyfin client compatibility (Web, Android, iOS, TV)
- Docker image

## Contributing

Contributions are welcome! Please:

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/my-feature`)
3. Commit your changes (`git commit -m 'Add my feature'`)
4. Push to the branch (`git push origin feature/my-feature`)
5. Open a Pull Request

## License

This project is open source. See the [LICENSE](LICENSE) file for details.

## Acknowledgments

- [Jellyfin](https://jellyfin.org/) — The free software media system this project aims to be compatible with
- [Axum](https://github.com/tokio-rs/axum) — Web framework
- [sqlx](https://github.com/launchbadge/sqlx) — Async SQL toolkit
- [Tokio](https://tokio.rs/) — Async runtime
