#!/usr/bin/env bash
# Differential conformance run (design §24.1): a fresh locus node and the
# pinned reference PDS, both on a shared mock PLC, driven through the same
# scenarios. Needs docker. Prints each step; exits non-zero on any
# difference the allowlist doesn't justify. Both proxy app.bsky.* to a mock
# AppView. MAIL=1 gives both a mail catcher (Mailpit, MAIL_SMTP and
# MAIL_API), so email flows run end to end; otherwise neither has SMTP.
set -uo pipefail

export PATH="$HOME/.cargo/bin:$PATH"
cd "$(dirname "$0")/.."
REF_IMAGE="${REF_IMAGE:-ghcr.io/bluesky-social/pds:0.4.5051}" # @atproto/pds 0.5.51
PLC_PORT=2592 APPVIEW_PORT=2593 LOCUS_PORT=3971 REF_PORT=3972
APPVIEW_DID=did:web:appview.test
MAIL_SMTP="${MAIL_SMTP:-smtp://127.0.0.1:55025}" MAIL_API="${MAIL_API:-http://127.0.0.1:55080}"
LOCUS_FROM=locus@conformance.test REF_FROM=reference@conformance.test
REF_MAIL=() LOCUS_MAIL=() MAIL_ARGS=()
if [ -n "${MAIL:-}" ]; then
  REF_MAIL=(-e PDS_EMAIL_SMTP_URL="$MAIL_SMTP" -e PDS_EMAIL_FROM_ADDRESS="$REF_FROM")
  LOCUS_MAIL=(PDS_EMAIL_SMTP_URL="$MAIL_SMTP" PDS_EMAIL_FROM_ADDRESS="$LOCUS_FROM")
  MAIL_ARGS=(--mail "$MAIL_API" --mail-from-locus "$LOCUS_FROM" --mail-from-reference "$REF_FROM")
fi
D=$(mktemp -d /tmp/locus-conformance.XXXX)
REF=locus-conformance-ref
PIDS=()
cleanup() {
  for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null; done
  docker rm -f "$REF" >/dev/null 2>&1
  docker run --rm -v "$D:/d" alpine:3 rm -rf /d/ref >/dev/null 2>&1
  rm -rf "$D"
}
trap cleanup EXIT

cargo build --quiet -p aurora-locus -p locus-conformance || exit 1
BIN=target/debug

"$BIN/locus-conformance" plc "$PLC_PORT" &
PIDS+=($!)
"$BIN/locus-conformance" appview "$APPVIEW_PORT" &
PIDS+=($!)

# The reference: host network, so it reaches the mock PLC on loopback.
docker rm -f "$REF" >/dev/null 2>&1
mkdir -p "$D/ref/blocks"
chmod -R 777 "$D/ref"
docker run -d --name "$REF" --network host -v "$D/ref:/pds" \
  -e PDS_HOSTNAME=localhost -e PDS_PORT="$REF_PORT" -e PDS_DEV_MODE=true \
  -e PDS_JWT_SECRET=0123456789abcdef0123456789abcdef -e PDS_ADMIN_PASSWORD=admin-pw \
  -e PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX=8f2a559490c4c8c9e8b8a4d8f7c3b3e1a2d4c6b8e0f1a3c5d7e9f0b2c4d6e8f1 \
  -e PDS_DATA_DIRECTORY=/pds -e PDS_BLOBSTORE_DISK_LOCATION=/pds/blocks \
  -e PDS_DID_PLC_URL="http://localhost:$PLC_PORT" -e PDS_SERVICE_HANDLE_DOMAINS=.test -e PDS_BLOB_UPLOAD_LIMIT=104857600 \
  -e PDS_INVITE_REQUIRED=false -e PDS_CRAWLERS= -e LOG_ENABLED=false \
  -e PDS_BSKY_APP_VIEW_URL="http://localhost:$APPVIEW_PORT" -e PDS_BSKY_APP_VIEW_DID="$APPVIEW_DID" \
  "${REF_MAIL[@]}" "$REF_IMAGE" >/dev/null || exit 1

env "${LOCUS_MAIL[@]}" PDS_BSKY_APP_VIEW_URL="http://localhost:$APPVIEW_PORT" PDS_BSKY_APP_VIEW_DID="$APPVIEW_DID" \
  LOCUS_DATA_DIR="$D/locus" PDS_HOSTNAME=localhost PDS_DEV_MODE=true LOCUS_PORT="$LOCUS_PORT" \
  LOCUS_SETUP_AUTO=true LOCUS_SETUP_OPERATOR_USERNAME=op LOCUS_SETUP_OPERATOR_PASSWORD=op-password-123 \
  LOCUS_SETUP_KEYS_BACKUP_ACKNOWLEDGED=true PDS_ADMIN_PASSWORD=admin-pw PDS_SERVICE_HANDLE_DOMAINS=.test PDS_BLOB_UPLOAD_LIMIT=104857600 \
  PDS_DID_PLC_URL="http://localhost:$PLC_PORT" PDS_INVITE_REQUIRED=false PDS_CRAWLERS= LOCUS_LOG_FORMAT=json \
  "$BIN/aurora-locus" serve >>"$D/locus.log" 2>&1 &
PIDS+=($!)

up() { for _ in $(seq 1 120); do curl -fsS "$1/xrpc/_health" >/dev/null 2>&1 && return 0; sleep 0.5; done; return 1; }
up "http://127.0.0.1:$REF_PORT" || { echo "the reference didn't start"; docker logs "$REF" | tail -20; exit 1; }
for _ in $(seq 1 80); do grep -q "starting again" "$D/locus.log" && break; sleep 0.5; done
up "http://127.0.0.1:$LOCUS_PORT" || { echo "locus didn't start"; tail -20 "$D/locus.log"; exit 1; }
sleep 2

"$BIN/locus-conformance" run \
  --locus "http://localhost:$LOCUS_PORT" --reference "http://localhost:$REF_PORT" \
  --allowlist conformance/allowlist.json --plc "http://localhost:$PLC_PORT" "${MAIL_ARGS[@]}" --report "${REPORT:-/tmp/locus-conformance-report.json}"
status=$?
if [ "$status" != 0 ] && [ -n "${SHOW_LOGS:-}" ]; then
  echo "--- locus log (warnings and errors)"
  grep -E '"level":"(WARN|ERROR)"' "$D/locus.log" | tail -20
fi
exit "$status"
