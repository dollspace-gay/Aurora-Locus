# aurora-locus: build the binary, then a slim runtime image with it (and
# the goat CLI, as the reference PDS image ships), running as a non-root
# user. Build from the workspace directory:
#
#   docker build -f deploy/docker/Dockerfile -t aurora-locus .

FROM rust:1.88-bookworm AS build
# kryphocron-lexicons generates its code at build time with this.
RUN cargo install proto-blue-codegen --version 0.3.3 --locked
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p aurora-locus

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
COPY --from=build /src/target/release/aurora-locus /usr/local/bin/aurora-locus
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
