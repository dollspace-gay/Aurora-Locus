#!/usr/bin/env bash
# Refreshes crates/locus-http/data/disposable_email_domains.txt from the
# disposable-email-domains-js package (CC0), the list the reference PDS
# uses. Usage: scripts/update-disposable-domains.sh [version]
set -euo pipefail

cd "$(dirname "$0")/.."
version="${1:-latest}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

meta="$(curl -sfL "https://registry.npmjs.org/disposable-email-domains-js/$version")"
tarball="$(printf '%s' "$meta" | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["dist"]["tarball"])')"
curl -sfL -o "$work/pkg.tgz" "$tarball"
tar xzf "$work/pkg.tgz" -C "$work"
python3 -I - "$work/package/dist/dict/disposable_email_blocklist.json" \
  crates/locus-http/data/disposable_email_domains.txt <<'PY'
import json, re, sys
domains = sorted({d.strip().lower() for d in json.load(open(sys.argv[1])) if d.strip()})
bad = [d for d in domains if not re.fullmatch(r"[a-z0-9.-]+", d)]
if bad:
    sys.exit(f"unexpected entries: {bad[:5]}")
open(sys.argv[2], "w").write("".join(d + "\n" for d in domains))
print(len(domains), "domains")
PY
