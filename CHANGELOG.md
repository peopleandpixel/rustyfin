# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.0] - 2026-10-29

### Added
- **Cross-platform release builds**: GitHub Actions workflow (`release.yml`) uses `cross` to build binaries for 5 platforms (Linux x86_64/aarch64, Windows x86_64, macOS x86_64/aarch64)
- **Release artifacts**: Each binary is packaged with the embedded web client for standalone distribution

### Changed
- **Code formatting**: Applied `cargo fmt` to fix line-length violations across API and DB crates

### Other
- **Build script** (`build.sh`): Automated build pipeline with Quality Gates (fmt, clippy, test), WebUI build, and release build
- **systemd service** (`rustyfin.service`): Production-ready service file with hardening (ProtectHome, ProtectSystem, NoNewPrivileges)
- **One-command install**: `sudo ./build.sh --install` installs binary, creates system user, and sets up systemd service

## [0.4.0] - 2026-10-28

### Added
- **Extended metadata from TMDb**: Movie and TV details now include status, original language, tagline, production countries, budget, revenue, homepage, vote count, and popularity
- **Studios from TMDb**: Studios are now fetched and stored in `studios`/`item_studios` tables
- **People for all media types**: Cast and crew data is now loaded for movies, episodes, and series (not just audio)
- **People batch query**: Batch enrichment now loads all people in a single query instead of N+1
- **Service Worker**: Offline caching for static web assets (JS, CSS, images)
- **Infinite Scroll**: Automatic loading when scrolling near the bottom, replacing "Load More" buttons
- **Virtual Scrolling**: Only visible cards are rendered in the library view (optimized for 1500+ items)
- **Vite build system**: Hot-reload development server (`npm run dev`) and optimized production builds

### Changed
- **WebUI performance optimizations**:
  - HLS.js loaded locally instead of from CDN (~200-500ms faster startup)
  - Image requests optimized: full-resolution images loaded on hover only
  - DOM rendering uses `DocumentFragment` instead of `innerHTML`
  - Template caching for item cards (~30% faster rendering)
  - Search input debounced (300ms delay)
  - CSS `backdrop-filter` only on desktop (mobile performance)
  - Hardware-accelerated CSS animations with `will-change`
- **Cache version automation**: MD5 hash of all assets is automatically computed on build and injected into Service Worker and HTML
- **Migration 025**: New DB columns for extended metadata (status, budget, revenue, homepage, vote_count, popularity)

### Fixed
- **Movie detail page**: Missing backdrop image, cast, and studio data
- **Similar recommendations**: Inconsistent card sizes (mixed movies and episodes in same row)
- **Test compilation**: `hash_combine` and `filter_cache_key` now accessible from test module
- **API tests**: Missing `http_client` field in test `AppState`
- **Migration 025**: Now registered in `lib.rs` so test databases include new columns

### Removed
- **Manual build script**: `build.js` replaced by Vite configuration (`vite.config.mjs`)

### Other
- **Code quality**: All `cargo fmt`, `cargo clippy`, and `cargo test` gates pass with zero warnings
- **Clippy allowances**: Added `#[allow(dead_code)]` for TMDb deserialization structs and `#[allow(clippy::too_many_arguments)]` for SQLx query functions
