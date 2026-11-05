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

did=$(echo "$acct" | python3 -c 'import json,sys; print(json.load(sys.stdin)["did"])')
x POST createRecord -H "authorization: Bearer $access" -H 'content-type: application/json' \
  -d "{\"repo\":\"$did\",\"collection\":\"app.bsky.feed.post\",\"rkey\":\"3m3smokeaaaaa\",\"record\":{\"text\":\"hello from smoke\",\"createdAt\":\"2026-11-05T00:00:00.000Z\"}}" \
  "$base/com.atproto.repo.createRecord"
x GET getRecord "$base/com.atproto.repo.getRecord?repo=$did&collection=app.bsky.feed.post&rkey=3m3smokeaaaaa"
printf '\x89PNG\r\n\x1a\n\0\0\0\rIHDRsmoke-image-bytes' >"$data/img.png"
blob=$(curl -s -H "authorization: Bearer $access" -H 'content-type: image/png' \
  --data-binary "@$data/img.png" "$base/com.atproto.repo.uploadBlob")
echo "> POST uploadBlob"; echo "$blob"
blobref=$(echo "$blob" | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin)["blob"]))')
cid=$(echo "$blob" | python3 -c 'import json,sys; print(json.load(sys.stdin)["blob"]["ref"]["$link"])')
x POST createRecord-image -H "authorization: Bearer $access" -H 'content-type: application/json' \
  -d "{\"repo\":\"$did\",\"collection\":\"app.bsky.feed.post\",\"record\":{\"text\":\"pic\",\"createdAt\":\"2026-11-05T00:00:00.000Z\",\"embed\":{\"\$type\":\"app.bsky.embed.images\",\"images\":[{\"image\":$blobref,\"alt\":\"\"}]}}}" \
  "$base/com.atproto.repo.createRecord"
echo "> GET getBlob (bytes match: $(curl -s "$base/com.atproto.sync.getBlob?did=$did&cid=$cid" | cmp -s - "$data/img.png" && echo yes || echo no))"
echo "> stored under: $(find "$data/blobs" -type f | sed "s|$data/||")"

# Export, change, import the export back.
curl -s "$base/com.atproto.sync.getRepo?did=$did" -o "$data/repo.car"
echo "> getRepo: $(wc -c <"$data/repo.car") bytes"
x POST deleteRecord -H "authorization: Bearer $access" -H 'content-type: application/json' \
  -d "{\"repo\":\"$did\",\"collection\":\"app.bsky.feed.post\",\"rkey\":\"3m3smokeaaaaa\"}" \
  "$base/com.atproto.repo.deleteRecord"
x POST importRepo -H "authorization: Bearer $access" -H 'content-type: application/vnd.ipld.car' \
  --data-binary "@$data/repo.car" "$base/com.atproto.repo.importRepo"
sleep 0.5
x GET getImportStatus -H "authorization: Bearer $access" "$base/app.nearhorizon.locus.account.getImportStatus"
x GET getRecord-restored "$base/com.atproto.repo.getRecord?repo=$did&collection=app.bsky.feed.post&rkey=3m3smokeaaaaa"
x GET listMissingBlobs -H "authorization: Bearer $access" "$base/com.atproto.repo.listMissingBlobs"

# The firehose from the start: a minimal WebSocket client (stdlib only)
# that reads frames for two seconds and lists their event types.
sleep 1
echo "> firehose from cursor=0"
python3 -I - "$port" <<'PY'
import base64, os, socket, sys, time
s = socket.create_connection(("127.0.0.1", int(sys.argv[1])))
key = base64.b64encode(os.urandom(16)).decode()
s.sendall((f"GET /xrpc/com.atproto.sync.subscribeRepos?cursor=0 HTTP/1.1\r\nHost: localhost\r\n"
           f"Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\n"
           f"Sec-WebSocket-Version: 13\r\n\r\n").encode())
buf = b""
while b"\r\n\r\n" not in buf:
    buf += s.recv(4096)
head, buf = buf.split(b"\r\n\r\n", 1)
print(head.split(b"\r\n")[0].decode())
s.settimeout(0.2)
end, kinds = time.time() + 2, []
while time.time() < end:
    try:
        chunk = s.recv(65536)
        if not chunk:
            break
        buf += chunk
    except socket.timeout:
        pass
    while len(buf) >= 2:
        n, off = buf[1] & 0x7F, 2
        if n == 126: n, off = int.from_bytes(buf[2:4], "big"), 4
        elif n == 127: n, off = int.from_bytes(buf[2:10], "big"), 10
        if len(buf) < off + n: break
        payload, buf = buf[off:off + n], buf[off + n:]
        for k in (b"#identity", b"#account", b"#commit", b"#sync", b"#info"):
            if k in payload[:40]:
                kinds.append(k.decode()); break
print(len(kinds), "events:", " ".join(kinds))
PY

# A handle elsewhere on the network, through DNS or HTTPS.
x GET resolveHandle-network "$base/com.atproto.identity.resolveHandle?handle=atproto.com"

kill -TERM "$pid"; wait "$pid" && echo "exit code: 0"
echo "--- log ---"; cat "$data/log"
