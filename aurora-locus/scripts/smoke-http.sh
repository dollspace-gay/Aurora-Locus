#!/usr/bin/env bash
# Boots `aurora-locus serve` on a fresh SQLite data dir with a throwaway mock
# PLC directory, then drives the account and session endpoints with curl.
set -euo pipefail

export PATH="$HOME/.cargo/bin:$PATH"
cd "$(dirname "$0")/.."
cargo build --quiet --bin aurora-locus

data="$(mktemp -d)"
port=2599
plc_port=2598

# A mock directory: accepts any POSTed operation, then serves it back on GET.
cat >"$data/plc.py" <<'PY'
import http.server, json, sys
ops = {}
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        ops[self.path] = self.rfile.read(int(self.headers["content-length"]))
        self.send_response(200); self.end_headers()
    def do_GET(self):
        body = ops.get(self.path.removesuffix("/log/last"))
        self.send_response(200 if body else 404)
        self.send_header("content-type", "application/json"); self.end_headers()
        self.wfile.write(body or b"{}")
    def log_message(self, *a): pass
http.server.HTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
PY
python3 "$data/plc.py" "$plc_port" &
plc_pid=$!

# Email goes to a local test SMTP server with an HTTP API, if one is given
# (e.g. Mailpit: SMOKE_SMTP_URL=smtp://127.0.0.1:55025 SMOKE_MAIL_API=http://127.0.0.1:55080).
LOCUS_DATA_DIR="$data" LOCUS_PORT="$port" PDS_HOSTNAME=localhost PDS_DEV_MODE=true \
  PDS_SERVICE_HANDLE_DOMAINS=.test PDS_INVITE_REQUIRED=false \
  PDS_EMAIL_SMTP_URL="${SMOKE_SMTP_URL:-}" PDS_EMAIL_FROM_ADDRESS="Smoke PDS <noreply@localhost.test>" \
  PDS_DID_PLC_URL="http://127.0.0.1:$plc_port" LOG_LEVEL=info \
  ./target/debug/aurora-locus serve >"$data/log" 2>&1 &
pid=$!
trap 'kill -TERM $pid $plc_pid 2>/dev/null || true; rm -rf "$data"' EXIT
for _ in $(seq 50); do curl -sf "localhost:$port/xrpc/_health" >/dev/null && break; sleep 0.2; done

x() { echo "> $1 $2"; shift 2; curl -s -w ' [%{http_code} in %{time_total}s]' "$@"; echo; }
base="http://localhost:$port/xrpc"
x GET health "$base/_health"
x GET describeServer "$base/com.atproto.server.describeServer"
acct=$(curl -s -H 'content-type: application/json' \
  -d '{"handle":"alice.test","email":"alice@example.com","password":"smoke-pw-1"}' \
  "$base/com.atproto.server.createAccount")
echo "> POST createAccount"; echo "$acct"
access=$(echo "$acct" | python3 -c 'import json,sys; print(json.load(sys.stdin)["accessJwt"])')
x GET getSession -H "authorization: Bearer $access" "$base/com.atproto.server.getSession"
x POST createSession -H 'content-type: application/json' \
  -d '{"identifier":"alice.test","password":"smoke-pw-1"}' "$base/com.atproto.server.createSession"
x POST createSession-bad -H 'content-type: application/json' \
  -d '{"identifier":"alice.test","password":"wrong"}' "$base/com.atproto.server.createSession"
x GET resolveHandle "$base/com.atproto.identity.resolveHandle?handle=alice.test"
x GET atproto-did -H 'host: alice.test' "http://localhost:$port/.well-known/atproto-did"
x GET did.json "http://localhost:$port/.well-known/did.json"

x POST updateHandle -H "authorization: Bearer $access" -H 'content-type: application/json' \
  -d '{"handle":"alice2.test"}' "$base/com.atproto.identity.updateHandle"
x GET resolveHandle-old "$base/com.atproto.identity.resolveHandle?handle=alice.test"
x GET resolveHandle-new "$base/com.atproto.identity.resolveHandle?handle=alice2.test"
x GET getSession-after -H "authorization: Bearer $access" "$base/com.atproto.server.getSession"

x POST createAccount-web -H 'content-type: application/json' \
  -d '{"handle":"wendy.test","email":"wendy@example.com","password":"smoke-pw-2","locusDidMethod":"web"}' \
  "$base/com.atproto.server.createAccount"
x GET did.json-web -H 'host: wendy.test' "http://localhost:$port/.well-known/did.json"

x POST requestEmailConfirmation -X POST -H "authorization: Bearer $access" \
  "$base/com.atproto.server.requestEmailConfirmation"
if [ -n "${SMOKE_MAIL_API:-}" ]; then
  sleep 1
  echo "> mail received"
  curl -s "$SMOKE_MAIL_API/api/v1/messages" | python3 -c '
import json, sys
for m in json.load(sys.stdin)["messages"][:1]:
    print(m["Subject"], "| to", [t["Address"] for t in m["To"]], "| from", m["From"]["Address"], "|", m["Snippet"][:90])'
fi

# A handle elsewhere on the network, through DNS or HTTPS.
x GET resolveHandle-network "$base/com.atproto.identity.resolveHandle?handle=atproto.com"

kill -TERM "$pid"; wait "$pid" && echo "exit code: 0"
echo "--- log ---"; cat "$data/log"
