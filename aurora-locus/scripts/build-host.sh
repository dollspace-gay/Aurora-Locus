#!/usr/bin/env bash
# Runs a cargo command for the workspace on the build host.
# Usage (from the dev machine, after syncing the workspace):
#   ssh <host> "bash ~/build/aurora-locus/scripts/build-host.sh <cargo args...>"
# Examples: check --workspace --all-targets | test --workspace | clippy --workspace -- -D warnings
set -euo pipefail

export PATH="$HOME/.cargo/bin:$PATH"
cd "$(dirname "$0")/.."

if [ "$#" -eq 0 ]; then
  set -- check --workspace --all-targets
fi

exec cargo "$@"
