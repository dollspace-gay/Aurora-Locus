# aurora-locus: build the binary, then a slim runtime image with it (and
# the goat CLI, as the reference PDS image ships), running as a non-root
# user. Build from the workspace directory:
#
#   docker build -f deploy/docker/Dockerfile -t aurora-locus .

FROM rust:1.88-bookworm AS build
# kryphocron-lexicons generates its code at build time with this.
RUN cargo install proto-blue-codegen --version 0.3.3 --locked
WORKDIR /src
# The pinned toolchain, in its own layer so builds don't fetch it again.
COPY rust-toolchain.toml .
RUN cargo --version
COPY . .
# Cargo's downloads and build output stay in BuildKit caches between
# builds, so a rebuild compiles the dependencies only when they change.
# The workspace's own sources are touched first: a copied layer Docker
# reuses can carry timestamps older than the cached build, and cargo
# would then keep a stale binary (a rollback that doesn't roll back). The
# cache isn't part of the image: the binary is copied out of it in the
# same step.
# Built on a server, the release profile's link-time optimization is off,
# which roughly halves a rebuild after a small change. For a
# fully optimized image (as a published one), build with
#   --build-arg CARGO_PROFILE_RELEASE_LTO=thin --build-arg CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
ARG CARGO_PROFILE_RELEASE_LTO=false
ARG CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16
RUN --mount=type=cache,id=locus-cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=locus-cargo-git,target=/usr/local/cargo/git \
    --mount=type=cache,id=locus-target,target=/src/target \
    find crates -type f -exec touch {} + \
    && cargo build --release --locked -p aurora-locus \
    && cp target/release/aurora-locus /usr/local/bin/aurora-locus

FROM golang:1.24-bookworm AS goat
# A goat release (github.com/bluesky-social/goat). Go fetches the toolchain
# the release asks for.
ARG GOAT_VERSION=v0.2.8
ENV GOTOOLCHAIN=auto
RUN CGO_ENABLED=0 go install github.com/bluesky-social/goat@${GOAT_VERSION}

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates tzdata \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --user-group --home-dir /data --shell /usr/sbin/nologin locus \
    && mkdir -p /data \
    && chown locus:locus /data \
    && chmod 0700 /data
COPY --from=build /usr/local/bin/aurora-locus /usr/local/bin/aurora-locus
COPY --from=goat /go/bin/goat /usr/local/bin/goat
USER locus
ENV LOCUS_DATA_DIR=/data \
    LOCUS_DATA_PERSISTENT=true \
    LOCUS_PORT=3000 \
    LOCUS_LOG_FORMAT=json
VOLUME /data
EXPOSE 3000
# /xrpc/_health, which answers in setup mode too (never /readyz).
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD ["aurora-locus", "healthcheck"]
ENTRYPOINT ["aurora-locus"]
CMD ["serve"]
