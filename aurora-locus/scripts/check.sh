#!/usr/bin/env bash
# Formats, lints and tests the workspace. Store tests also run on Postgres
# through LOCUS_TEST_POSTGRES_URL (defaults to the local test container).
set -euo pipefail

export PATH="$HOME/.cargo/bin:$PATH"
cd "$(dirname "$0")/.."
export LOCUS_TEST_POSTGRES_URL="${LOCUS_TEST_POSTGRES_URL:-postgres://locus:locus@127.0.0.1:55491/locus}"

cargo fmt --all
cargo clippy --workspace --all-targets --quiet --message-format short -- -D warnings
set +o pipefail
cargo test --workspace --quiet 2>&1 | grep -E 'test result|FAILED|panicked|^error'
exit "${PIPESTATUS[0]}"
