# ---- Build Stage ----
FROM rust:1.85-bookworm AS builder

WORKDIR /usr/src/rustyfin

# Abhängigkeiten cachen: erst nur Cargo-Dateien kopieren
COPY Cargo.toml Cargo.lock ./
COPY crates/rustyfin-server/Cargo.toml crates/rustyfin-server/Cargo.toml
COPY crates/rustyfin-api/Cargo.toml crates/rustyfin-api/Cargo.toml
COPY crates/rustyfin-auth/Cargo.toml crates/rustyfin-auth/Cargo.toml
COPY crates/rustyfin-core/Cargo.toml crates/rustyfin-core/Cargo.toml
COPY crates/rustyfin-db/Cargo.toml crates/rustyfin-db/Cargo.toml
COPY crates/rustyfin-model/Cargo.toml crates/rustyfin-model/Cargo.toml
COPY crates/rustyfin-cli/Cargo.toml crates/rustyfin-cli/Cargo.toml

# Dummy-Dateien für Dependency-Cache
RUN mkdir -p crates/rustyfin-server/src && echo "fn main() {}" > crates/rustyfin-server/src/main.rs && \
    mkdir -p crates/rustyfin-api/src && echo "" > crates/rustyfin-api/src/lib.rs && \
    mkdir -p crates/rustyfin-auth/src && echo "" > crates/rustyfin-auth/src/lib.rs && \
    mkdir -p crates/rustyfin-core/src && echo "" > crates/rustyfin-core/src/lib.rs && \
    mkdir -p crates/rustyfin-db/src && echo "" > crates/rustyfin-db/src/lib.rs && \
    mkdir -p crates/rustyfin-model/src && echo "" > crates/rustyfin-model/src/lib.rs && \
    mkdir -p crates/rustyfin-cli/src && echo "" > crates/rustyfin-cli/src/lib.rs

RUN cargo build --release 2>/dev/null || true

# Quellcode kopieren und bauen
COPY crates/ crates/
RUN touch crates/*/src/*.rs && \
    cargo build --release

# ---- Runtime Stage ----
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates curl && \
    rm -rf /var/lib/apt/lists/*

RUN useradd -r -s /bin/false -m -d /data rustyfin

COPY --from=builder /usr/src/rustyfin/target/release/rustyfin /usr/local/bin/rustyfin

USER rustyfin
WORKDIR /data

ENV RUSTYFIN_DATA=/data
ENV RUSTYFIN_BIND=0.0.0.0:8096
ENV RUST_LOG=info

EXPOSE 8096

HEALTHCHECK --interval=30s --timeout=10s --retries=3 \
    CMD curl -f http://localhost:8096/System/Info/Public || exit 1

ENTRYPOINT ["rustyfin"]
