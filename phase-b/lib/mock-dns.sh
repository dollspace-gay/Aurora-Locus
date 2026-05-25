# phase-b/lib/mock-dns.sh — launch/teardown wrapper for the Phase B
# DNS-TXT authority responder (Cluster 1 Member 1.2 / Scenario 13).
#
# Source this file. Provides:
#
#   pb_mock_dns_start <config-path>  # cargo run --bin phase-b-dns-responder ...
#   pb_mock_dns_stop                 # kill the launched responder
#   pb_mock_dns_wait                 # bounded retry until the socket binds
#
# OPERATOR-harness launcher only — sibling shape to lib/mock-plc.sh.
# Scenario 13's responder is the only consumer; no other arc needs DNS
# authority injection.
#
# Default bind: 127.0.0.1:5353 (matches the env-var injection target
# the resolver constructor expects). Override via $MOCK_DNS_BIND if a
# scenario reserves a different address.
#
# `--self-test` is NOT used by the harness — that's a CC-side compile
# sanity for the encode path, not Phase B judgment.

set -u

: "${MOCK_DNS_BIND:=127.0.0.1:5353}"

PB_MOCK_DNS_PID_PATH="/tmp/pb-mock-dns.pid"
PB_MOCK_DNS_LOG_PATH="/tmp/pb-mock-dns.log"

# -----------------------------------------------------------------------------
# pb_mock_dns_start <config-path>
#
# Launches the responder via `cargo run --bin phase-b-dns-responder`
# (debug build; Phase B convention is debug, not --release). Idempotent
# if the responder is already running on the bound port.
# -----------------------------------------------------------------------------

pb_mock_dns_start() {
    local config_path="${1:-}"
    if [ -z "$config_path" ]; then
        echo "[pb-mock-dns] usage: pb_mock_dns_start <config-path>" >&2
        return 1
    fi
    if [ ! -r "$config_path" ]; then
        echo "[pb-mock-dns] config not readable: $config_path" >&2
        return 1
    fi

    if [ -r "$PB_MOCK_DNS_PID_PATH" ]; then
        local existing
        existing=$(cat "$PB_MOCK_DNS_PID_PATH")
        if kill -0 "$existing" 2>/dev/null; then
            echo "[pb-mock-dns] already running pid=$existing on $MOCK_DNS_BIND"
            return 0
        fi
        # stale pid file
        rm -f "$PB_MOCK_DNS_PID_PATH"
    fi

    # cargo run from the repo root so the binary target resolves.
    # Redirect, NOT tee — same convention as the rest of the harness.
    cargo run --bin phase-b-dns-responder -- \
        --bind "$MOCK_DNS_BIND" \
        --config "$config_path" \
        >"$PB_MOCK_DNS_LOG_PATH" 2>&1 &
    echo $! >"$PB_MOCK_DNS_PID_PATH"

    echo "[pb-mock-dns] launched on $MOCK_DNS_BIND  pid=$(cat $PB_MOCK_DNS_PID_PATH)  log=$PB_MOCK_DNS_LOG_PATH"
}

# -----------------------------------------------------------------------------
# pb_mock_dns_wait
#
# Bounded retry until the responder's UDP socket is reachable. We
# can't curl a UDP socket the way mock-plc.sh curls /_health on TCP,
# so we probe by issuing a DNS query and accepting any response (the
# responder's own log line "listening on ..." is the secondary check
# operators can scan).
# -----------------------------------------------------------------------------

pb_mock_dns_wait() {
    local i
    for i in $(seq 1 30); do
        if grep -q "listening on" "$PB_MOCK_DNS_LOG_PATH" 2>/dev/null; then
            echo "[pb-mock-dns] ready on $MOCK_DNS_BIND  attempts=$i"
            return 0
        fi
        sleep 1
    done
    echo "[pb-mock-dns] NOT READY after 30s — log tail:" >&2
    tail -n 50 "$PB_MOCK_DNS_LOG_PATH" >&2 || true
    return 1
}

# -----------------------------------------------------------------------------
# pb_mock_dns_stop
# -----------------------------------------------------------------------------

pb_mock_dns_stop() {
    if [ -r "$PB_MOCK_DNS_PID_PATH" ]; then
        local pid
        pid=$(cat "$PB_MOCK_DNS_PID_PATH")
        if kill "$pid" 2>/dev/null; then
            echo "[pb-mock-dns] killed pid=$pid"
        fi
        rm -f "$PB_MOCK_DNS_PID_PATH"
    fi
}

:
