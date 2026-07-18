# syntax=docker/dockerfile:1

###############################################################################
# Builder — compiles the Dioxus fullstack web bundle (native Axum server +
# the hydrated WASM client). Mirrors the CI bundle step in
# .github/workflows/e2e.yml: `dx bundle --platform web --package omnibus
# --fullstack --release`, which emits target/dx/omnibus/release/web/.
#
# This deliberately does NOT use the Nix dev shell — a plain Rust toolchain
# keeps the image conventional and light. `dx` downloads the matching
# wasm-bindgen + wasm-opt itself, so the wasm-bindgen pin in flake.nix is not
# needed here. The Dioxus libraries are patched to the v0.7.9 git tag (see
# [patch.crates-io] in Cargo.toml), so the CLI is pinned to the same 0.7.9.
###############################################################################
# Debian 13 (trixie) for glibc >= 2.39: the prebuilt `dx` release binary is
# linked against GLIBC_2.39, which bookworm (2.36) doesn't provide. The runtime
# stage must share this glibc baseline since the server binary is compiled here.
FROM rust:1-trixie AS builder

# `clang`/`pkg-config` cover the handful of -sys crates; `curl` is for the
# cargo-binstall bootstrap below. sqlite is statically bundled by
# libsqlite3-sys and TLS is rustls, so no libsqlite3/openssl dev packages.
RUN apt-get update && apt-get install -y --no-install-recommends \
        clang pkg-config curl \
    && rm -rf /var/lib/apt/lists/*

RUN rustup target add wasm32-unknown-unknown

# Pull the prebuilt Dioxus CLI release binary (compiling it from source would
# add many minutes). Pinned to v0.7.9 to match the patched Dioxus libraries.
RUN curl -L --proto '=https' --tlsv1.2 -sSf \
        https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.sh | bash \
    && cargo binstall -y dioxus-cli@0.7.9

WORKDIR /src
COPY . .

# Release tag (e.g. "v0.8.9") the crate's own `version` never carries — every
# merge to main cuts a release (.github/workflows/release.yml) but Cargo.toml
# stays pinned at 0.1.0. `frontend::version::app_version` reads this via
# `option_env!` at compile time, so it must be set before `dx bundle`
# compiles both the server binary and the WASM client (#1055).
ARG OMNIBUS_VERSION
ENV OMNIBUS_VERSION=${OMNIBUS_VERSION}

# Produces /src/target/dx/omnibus/release/web/{server, public/, ...}.
RUN dx bundle --platform web --package omnibus --fullstack --release

###############################################################################
# Runtime — slim image carrying just the bundle, ffmpeg (audiobook HLS
# transcode), kepubify (EPUB→KEPUB for the "Send to Kobo" download), CA certs
# (remote cover/author-photo fetches), curl (health probe), and gosu
# (privilege drop). Starts as root so the PUID/PGID entrypoint can remap the
# app user, then drops to it before serving.
###############################################################################
# Must match the builder's glibc (trixie) — the server binary is linked against it.
FROM debian:trixie-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
        ffmpeg ca-certificates curl gosu \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 1000 --user-group --create-home --shell /usr/sbin/nologin omnibus

# kepubify is a single Go binary, not a Debian package — fetch the release
# matching the image architecture. `db::kepub` invokes it on PATH (or via
# OMNIBUS_KEPUBIFY_PATH). Absence is non-fatal (falls back to plain EPUB), but
# ship it so the "Send to Kobo" download serves optimized KEPUB by default.
ARG KEPUBIFY_VERSION=v4.0.4
RUN set -eux; \
    arch="$(dpkg --print-architecture)"; \
    case "$arch" in \
        amd64) kbin=kepubify-linux-64bit ;; \
        arm64) kbin=kepubify-linux-arm64 ;; \
        *) echo "unsupported arch for kepubify: $arch" >&2; exit 1 ;; \
    esac; \
    curl -L --proto '=https' --tlsv1.2 -sSf \
        "https://github.com/pgaskin/kepubify/releases/download/${KEPUBIFY_VERSION}/${kbin}" \
        -o /usr/local/bin/kepubify; \
    chmod +x /usr/local/bin/kepubify; \
    kepubify --version

WORKDIR /app
# The server locates its `public/` assets relative to the binary, so keep the
# whole bundle directory intact.
COPY --from=builder /src/target/dx/omnibus/release/web/ /app/
COPY docker/entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh

# Re-declared in this stage (ARG/ENV don't cross the builder's `FROM`
# boundary) so the *running* server also reads its own release tag at boot
# — `server::backend::app_version` calls `std::env::var("OMNIBUS_VERSION")`
# at runtime, not compile time, unlike the frontend's `option_env!` constant
# baked in above (#1055).
ARG OMNIBUS_VERSION

# Bind on all interfaces (Dioxus defaults to 127.0.0.1, unreachable from
# outside the container) and default the persistent paths into the /config
# and /cache volumes. Everything here is overridable in docker-compose.yml.
ENV IP=0.0.0.0 \
    PORT=3000 \
    DATABASE_URL="sqlite:///config/omnibus.db?mode=rwc" \
    OMNIBUS_COVERS_DIR=/config/covers \
    OMNIBUS_THUMBS_DIR=/cache/thumbs \
    OMNIBUS_DATA_DIR=/cache/data \
    OMNIBUS_VERSION=${OMNIBUS_VERSION}

EXPOSE 3000
VOLUME ["/config", "/cache"]

HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD curl -fsS "http://127.0.0.1:${PORT}/api/_health" || exit 1

# Runs as root; the entrypoint applies PUID/PGID and drops to the omnibus user.
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
CMD ["/app/server"]
