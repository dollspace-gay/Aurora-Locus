# Aurora Locus PDS — production image.
#
# NOTE (housekeeping #421 B.5): this replaces a stale Dockerfile that pinned
# rust:1.75 (too old for the current dep tree — CI builds on stable), omitted
# the disk-served `static/` assets (holder + admin UI → 404 at runtime) and
# `build.rs` (build-info; build failure), and set the wrong port/env
# (`DATABASE_URL`/3000; the app reads `PDS_*` and listens on 2583).

# ---- builder ----
FROM rust:1.83-bookworm AS builder
WORKDIR /app

# openssl-sys (transitive) needs the system lib + pkg-config to build.
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Build inputs. `build.rs` (vergen build-info) and the `libs/` workspace member
# are required to compile; `migrations/` is embedded at compile time by the
# `sqlx::migrate!` macros, so it must be present in the builder.
COPY Cargo.toml Cargo.lock build.rs ./
COPY libs ./libs
COPY src ./src
COPY migrations ./migrations

RUN cargo build --release

# ---- runtime ----
FROM debian:bookworm-slim

# ca-certificates + libssl3 for TLS/crypto; curl for the compose healthcheck.
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libssl3 curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/aurora-locus /usr/local/bin/aurora-locus

# Runtime disk assets, copied from the build context (not build outputs).
# `static/` is served from disk via ServeDir (server.rs — /admin + /holder) and
# `lexicons/` is read at runtime; both are resolved relative to the working
# directory, so they must live under /app.
COPY static ./static
COPY lexicons ./lexicons

RUN mkdir -p /data

# The app listens on PDS_PORT (default 2583) and reads PDS_* config; only
# PDS_DATA_DIRECTORY is required (all component paths derive from it). Pass the
# rest (PDS_HOSTNAME, PDS_SERVICE_DID, signing keys, …) via the environment /
# an .env file (see .env.example) — never bake secrets into the image.
EXPOSE 2583
ENV RUST_LOG=info
ENV PDS_DATA_DIRECTORY=/data

CMD ["aurora-locus"]
