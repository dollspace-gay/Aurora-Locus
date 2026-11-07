#!/usr/bin/env bash
# Boots `aurora-locus serve` twice against a fresh SQLite data dir, stopping
# it with SIGTERM each time, and prints the logs. The first boot should apply
# the env value; the second should keep the same node id and apply nothing.
set -euo pipefail

export PATH="$HOME/.cargo/bin:$PATH"
cd "$(dirname "$0")/.."
cargo build --quiet --bin aurora-locus

data="$(mktemp -d)"
trap 'rm -rf "$data"' EXIT

run() {
  LOCUS_DATA_DIR="$data" PDS_SERVICE_NAME="Smoke PDS" LOG_LEVEL=info \
    ./target/debug/aurora-locus serve >"$data/log.$1" 2>&1 &
  local pid=$!
  sleep 3
  kill -TERM "$pid"
  wait "$pid"
  echo "exit code: $?"
  echo "--- boot $1 log ---"
  cat "$data/log.$1"
}

run 1
run 2
echo "--- node id file ---"
cat "$data/node-id"; echo
