#!/usr/bin/env bash
# Formats, lints and tests the workspace. Store tests also run on Postgres
# through LOCUS_TEST_POSTGRES_URL (defaults to the local test container).
set -euo pipefail

export PATH="$HOME/.cargo/bin:$PATH"
cd "$(dirname "$0")/.."
export LOCUS_TEST_POSTGRES_URL="${LOCUS_TEST_POSTGRES_URL:-postgres://locus:locus@127.0.0.1:55491/locus}"

# Each store test makes its own database; drop the ones earlier runs left.
if command -v docker >/dev/null && docker ps --format '{{.Names}}' | grep -qx locus-test-pg; then
  docker exec locus-test-pg psql -U locus -d locus -tAc \
    "SELECT datname FROM pg_database WHERE datname LIKE 'locus_test_%'" |
    while read -r db; do
      [ -n "$db" ] && docker exec locus-test-pg dropdb -U locus --if-exists "$db"
    done
fi

# KMS tests run when the local KMS container is up (local-kms takes any
# credentials).
running() { command -v docker >/dev/null && docker ps --format '{{.Names}}' | grep -qx "$1"; }
if running locus-test-kms; then
  export LOCUS_TEST_KMS_ENDPOINT=http://127.0.0.1:59100 LOCUS_KMS_ENDPOINT=http://127.0.0.1:59100
  export AWS_REGION=eu-west-2 AWS_ACCESS_KEY_ID="${AWS_ACCESS_KEY_ID:-locus}" AWS_SECRET_ACCESS_KEY="${AWS_SECRET_ACCESS_KEY:-locus-secret}"
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
exit "$status"
