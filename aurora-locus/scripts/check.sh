#!/usr/bin/env bash
# Formats, lints and tests the workspace. Store tests also run on Postgres
# through LOCUS_TEST_POSTGRES_URL (defaults to the local test container).
set -euo pipefail

export PATH="$HOME/.cargo/bin:$PATH"
cd "$(dirname "$0")/.."

# kryphocron-lexicons runs proto-blue-codegen from its build script.
command -v proto-blue-codegen >/dev/null || cargo install proto-blue-codegen --version 0.3.3 --locked

export LOCUS_TEST_POSTGRES_URL="${LOCUS_TEST_POSTGRES_URL:-postgres://locus:locus@127.0.0.1:55491/locus}"

# Each store test makes its own database; drop the ones earlier runs left.
if command -v docker >/dev/null && docker ps --format '{{.Names}}' | grep -qx locus-test-pg; then
  docker exec locus-test-pg psql -U locus -d locus -tAc \
    "SELECT datname FROM pg_database WHERE datname LIKE 'locus_test_%'" |
    while read -r db; do
      # One another run is still using stays (runs may overlap).
      if [ -n "$db" ]; then docker exec locus-test-pg dropdb -U locus --if-exists "$db" >/dev/null 2>&1 || true; fi
    done
fi

# KMS tests run when the local KMS container is up (local-kms takes any
# credentials).
running() { command -v docker >/dev/null && docker ps --format '{{.Names}}' | grep -qx "$1"; }
if running locus-test-kms; then
  export LOCUS_TEST_KMS_ENDPOINT=http://127.0.0.1:59100 LOCUS_KMS_ENDPOINT=http://127.0.0.1:59100
  export AWS_REGION=eu-west-2 AWS_ACCESS_KEY_ID="${AWS_ACCESS_KEY_ID:-locus}" AWS_SECRET_ACCESS_KEY="${AWS_SECRET_ACCESS_KEY:-locus-secret}"
fi

# Direct TLS tests start Pebble (an ACME test server) when its image is here.
if command -v docker >/dev/null && docker image inspect ghcr.io/letsencrypt/pebble:latest >/dev/null 2>&1; then
  export LOCUS_TEST_PEBBLE_IMAGE="${LOCUS_TEST_PEBBLE_IMAGE:-ghcr.io/letsencrypt/pebble:latest}"
fi

# Redis tests run when the local Redis container is up.
if running locus-test-redis; then
  export LOCUS_TEST_REDIS_URL=redis://127.0.0.1:56379
fi

cargo fmt --all
cargo clippy --workspace --all-targets --quiet --message-format short -- -D warnings
set +o pipefail
cargo test --workspace --quiet 2>&1 | grep -E 'test result|FAILED|panicked|^error'
status="${PIPESTATUS[0]}"
# The blob store's own tests again against the S3 container, when it's up.
if [ "$status" = 0 ] && running locus-test-s3; then
  LOCUS_TEST_S3_ENDPOINT=http://127.0.0.1:59000 LOCUS_TEST_S3_GATED_BUCKET=locus-test-gated \
    LOCUS_TEST_S3_VERSIONED_BUCKET=locus-test-versioned \
    AWS_ACCESS_KEY_ID=locus AWS_SECRET_ACCESS_KEY=locus-secret \
    cargo test -p locus-blob --quiet 2>&1 | grep -E 'test result|FAILED|panicked|^error'
  status="${PIPESTATUS[0]}"
fi
# The HTTP suite again on Postgres. The blobs, audiences and keys modules
# set up rows with SQLite SQL throughout, so they run on SQLite only (the
# store tests cover those paths on both backends, including the Postgres
# blob row lock); other tests that do so skip themselves.
if [ "$status" = 0 ] && [ -n "${LOCUS_TEST_POSTGRES_URL:-}" ]; then
  LOCUS_TEST_HTTP_POSTGRES_URL="$LOCUS_TEST_POSTGRES_URL" \
    cargo test -p locus-http --quiet -- --skip tests::blobs:: --skip tests::audiences:: --skip tests::keys:: \
    2>&1 | grep -E 'test result|FAILED|panicked|^error'
  status="${PIPESTATUS[0]}"
fi
exit "$status"
