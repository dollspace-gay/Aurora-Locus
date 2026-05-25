# phase-b/lib/mock-plc.sh — launch/teardown wrapper around mock-plc.py.
#
# Source this file. Provides:
#
#   pb_mock_plc_start    # python3 phase-b/mock-plc.py --port $MOCK_PLC_PORT &
#   pb_mock_plc_stop     # kill the launched mock-plc; idempotent
#   pb_mock_plc_wait     # bounded retry curl /_health until 200
#
# OPERATOR-harness launcher only. The CI mock-PLC launch step lives in
# .github/workflows/ci.yml and uses `python3 phase-b/mock-plc.py`
# directly (NOT this wrapper) — same script, different driver, different
# assertion philosophy. See V06_DESIGN.md Cluster 1 Member 1.4 for the
# "they share the script, not the launcher" framing.
#
# /_health note: this is `mock-plc.py`'s OWN /_health, which works. NOT
# the PDS /_health (which 404s per #97). Different services, different
# ports — mock-PLC on 2582, PDS on $PDS_SERVICE_PORT.

set -u

: "${MOCK_PLC_PORT:=2582}"

# pid file path — distinct from the PDS instance pid files so coexistence
# with two instances doesn't collide.
PB_MOCK_PLC_PID_PATH="/tmp/pb-mock-plc.pid"
PB_MOCK_PLC_LOG_PATH="/tmp/pb-mock-plc.log"

# -----------------------------------------------------------------------------
# pb_mock_plc_start
# Launches mock-plc.py on $MOCK_PLC_PORT (default 2582). Writes pid to
# $PB_MOCK_PLC_PID_PATH; log to $PB_MOCK_PLC_LOG_PATH.
# If a mock-plc is already responding on the port, this is a no-op (the
# launcher is idempotent across re-sources).
# -----------------------------------------------------------------------------

pb_mock_plc_start() {
    if curl -sf "http://localhost:${MOCK_PLC_PORT}/_health" >/dev/null 2>&1; then
        echo "[pb-mock-plc] already running on :${MOCK_PLC_PORT}"
        return 0
    fi

    # Repo-root-relative path; assumes scenarios run from the repo root.
    local script="phase-b/mock-plc.py"
    if [ ! -r "$script" ]; then
        echo "[pb-mock-plc] cannot find $script — run scenarios from the repo root" >&2
        return 1
    fi

    python3 "$script" --port "$MOCK_PLC_PORT" >"$PB_MOCK_PLC_LOG_PATH" 2>&1 &
    echo $! >"$PB_MOCK_PLC_PID_PATH"
    echo "[pb-mock-plc] launched on :${MOCK_PLC_PORT}  pid=$(cat $PB_MOCK_PLC_PID_PATH)  log=$PB_MOCK_PLC_LOG_PATH"
}

# -----------------------------------------------------------------------------
# pb_mock_plc_wait
# Bounded retry until /_health returns 2xx. Same shape as the CI yaml
# liveness check — load-bearing because the smoke test races otherwise.
# -----------------------------------------------------------------------------

pb_mock_plc_wait() {
    local i
    for i in $(seq 1 30); do
        if curl -sf "http://localhost:${MOCK_PLC_PORT}/_health" >/dev/null 2>&1; then
            echo "[pb-mock-plc] ready on :${MOCK_PLC_PORT}  attempts=$i"
            return 0
        fi
        sleep 1
    done
    echo "[pb-mock-plc] NOT READY after 30s — log tail:" >&2
    tail -n 50 "$PB_MOCK_PLC_LOG_PATH" >&2 || true
    return 1
}

# -----------------------------------------------------------------------------
# pb_mock_plc_stop
# Kill the launched mock-plc. Best-effort; absent pid file is no-op.
# -----------------------------------------------------------------------------

pb_mock_plc_stop() {
    if [ -r "$PB_MOCK_PLC_PID_PATH" ]; then
        local pid
        pid=$(cat "$PB_MOCK_PLC_PID_PATH")
        if kill "$pid" 2>/dev/null; then
            echo "[pb-mock-plc] killed pid=$pid"
        fi
        rm -f "$PB_MOCK_PLC_PID_PATH"
    fi
}

:
