#!/usr/bin/env bash
# Build the per-distro install.sh preflight smoke images and print a summary.
#
# Each image's build ASSERTS its distro's package-manager branch ran (the final
# RUN exits non-zero if the branch did not complete), so a successful `docker
# build` == that distro's preflight passed.
#
# The build context is a per-run temp dir holding just install.sh + .env.example
# (the two files the preflight needs). We do NOT build from the repo root: the
# top-level .dockerignore excludes install.sh (it isn't an input to the main
# docker-compose image), which would starve the COPY. Staging also keeps the
# context tiny and sidesteps .dockerignore entirely.
#
# NOTE: this stages the WORKING-TREE install.sh rather than `git clone`-ing —
# the current work branch is not pushed, so a clone would test a stale install.sh.
set -uo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
distros=(rocky alma arch)
declare -A result

for d in "${distros[@]}"; do
    echo "=================================================================="
    echo "  building install-sh-smoke:$d"
    echo "=================================================================="
    stage="$(mktemp -d)"
    cp "$repo_root/install.sh" "$repo_root/.env.example" "$stage/"
    if docker build \
            -f "$repo_root/test/install-sh-smoke/Dockerfile.$d" \
            -t "aurora-install-smoke:$d" \
            "$stage"; then
        result[$d]=PASS
    else
        result[$d]=FAIL
    fi
    rm -rf "$stage"
done

echo
echo "=== install.sh preflight smoke summary ==="
rc=0
for d in "${distros[@]}"; do
    printf '  %-6s %s\n' "$d" "${result[$d]}"
    [[ "${result[$d]}" == PASS ]] || rc=1
done
exit "$rc"
