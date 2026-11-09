# syntax=docker/dockerfile:1

# ---- Web Client Stage ----
FROM --platform=$BUILDPLATFORM node:24-slim AS web

WORKDIR /usr/src/rustyfin-web

# Corepack provides the pnpm version pinned in package.json (packageManager)
RUN corepack enable

COPY crates/rustyfin-web/package.json crates/rustyfin-web/pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile

COPY crates/rustyfin-web/ ./
RUN pnpm run build

# ---- Build Stage ----
FROM rust:1-trixie AS builder

WORKDIR /usr/src/rustyfin

COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/

# The cache mounts only speed up local rebuilds, so the binary is copied out of target/
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/src/rustyfin/target \
    cargo build --release --locked --bin rustyfin && \
    cp target/release/rustyfin /usr/local/bin/rustyfin

# ---- Runtime Base Stage ----
# Shared by the two images below, so they can't drift apart
FROM debian:trixie-slim AS runtime

ARG TARGETARCH

# ffmpeg for probing and transcoding, VA-API drivers for hardware acceleration
# (the Intel iHD driver is amd64-only and lives in non-free)
RUN set -eux; \
    if [ "$TARGETARCH" = amd64 ]; then \
        sed -i 's/^Components: main$/Components: main non-free non-free-firmware/' /etc/apt/sources.list.d/debian.sources; \
        intel=intel-media-va-driver-non-free; \
    fi; \
    apt-get update; \
    apt-get install -y --no-install-recommends ca-certificates curl ffmpeg mesa-va-drivers ${intel:-}; \
    rm -rf /var/lib/apt/lists/*

RUN useradd -r -s /bin/false -m -d /data rustyfin

USER rustyfin
WORKDIR /data

ENV RUSTYFIN_DATA=/data
ENV RUSTYFIN_BIND=0.0.0.0:8096
ENV RUSTYFIN_WEBCLIENT_DIR=/usr/local/share/rustyfin/web
ENV RUST_LOG=info

EXPOSE 8096

HEALTHCHECK --interval=30s --timeout=10s --retries=3 \
    CMD curl -f http://localhost:8096/System/Info/Public || exit 1

ENTRYPOINT ["rustyfin"]

# ---- CI Image (--target prebuilt) ----
# The binary and web client that CI built, downloaded to prebuilt/. Artifacts lose the
# executable bit, so the binary gets it back here.
FROM runtime AS prebuilt

COPY --chmod=755 prebuilt/rustyfin /usr/local/bin/rustyfin
COPY prebuilt/web /usr/local/share/rustyfin/web

# ---- Runtime Stage ----
# Built from source. It stays last, so a plain `docker build .` builds it.
FROM runtime

COPY --from=builder /usr/local/bin/rustyfin /usr/local/bin/rustyfin
COPY --from=web /usr/src/rustyfin-web/dist /usr/local/share/rustyfin/web
