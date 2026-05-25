# phase-b/lib/instance.sh — launch / wait-for-ready / kill helpers.
#
# Source this file. Provides:
#
#   pb_kill_prior              # pkill any lingering aurora-locus instance
#   pb_launch_instance <role>  # cargo run --bin aurora-locus > /tmp/pds-<role>-<backend>.log 2>&1 &
#   pb_wait_for_ready <role>   # poll describeServer until 200 (bounded retry)
#   pb_grep_banner   <role>    # confirm right port + right data dir in startup log
#
# Conventions (locked, do NOT change inside scenarios):
#   - Redirect via `>` + `2>&1`, NEVER `tee` (tee'd ANSI from
#     fmt::Layer.pretty() breaks quoted greps).
#   - NEVER `--release`. Phase B exercises debug-built behavior
#     including `debug!` emission.
#   - Wait-for-ready hits `describeServer` (NOT `/xrpc/_health`, which
#     404s per #97).
#   - Per-instance log path: /tmp/pds-<role>-<backend>.log so
#     dual-backend runs don't clobber each other.
#
# Setup-only-never-judgment: these helpers exit non-zero on a SETUP
# failure (process didn't launch, didn't reach ready, banner doesn't
# match the launching env). They do NOT make Phase B semantic
# assertions; that's the operator's, against captured wire output.

set -u

# -----------------------------------------------------------------------------
# pb_kill_prior — pkill any lingering aurora-locus instance.
# Best-effort; absent processes are not an error.
# -----------------------------------------------------------------------------

pb_kill_prior() {
    pkill -f "target/.*/aurora-locus" 2>/dev/null || true
    # Settle window — kill is async; subsequent launch may race the
    # port-bind without a brief wait.
    sleep 1
    echo "[pb-instance] killed any prior aurora-locus processes"
}

# -----------------------------------------------------------------------------
# pb_launch_instance <role>
# Expects role-env already emitted (A_ENV/B_ENV path set + file readable)
# and the per-role data dir pre-created (call lib/data.sh helpers first).
# Logs to /tmp/pds-<role>-<backend>.log (redirect, NOT tee).
# -----------------------------------------------------------------------------

pb_launch_instance() {
    local role="$1"
    local upper
    upper=$(echo "$role" | tr '[:lower:]' '[:upper:]')
    local env_var="${upper}_ENV"
    local env_path="${!env_var:-}"
    local backend="${BACKEND:-sqlite}"
    local log_path="/tmp/pds-${role}-${backend}.log"

    if [ -z "$env_path" ] || [ ! -r "$env_path" ]; then
        echo "[pb-instance] env file missing for role=$role ($env_var='$env_path')" >&2
        return 1
    fi

    # Subshell so the source doesn't leak the per-role env into the
    # parent shell (parent keeps its own state; the launched process
    # gets the right env via this subshell's exec).
    (
        set -a
        # shellcheck disable=SC1090
        source "$env_path"
        set +a
        # Redirect, NOT tee. NOT --release.
        # `--bin aurora-locus` is load-bearing: M1.2(b) (commit 6bfa24a)
        # added phase-b-dns-responder as a second [[bin]] in Cargo.toml,
        # so bare `cargo run` is now ambiguous and fails to launch.
        cargo run --bin aurora-locus >"$log_path" 2>&1 &
        echo $! > "/tmp/pds-${role}-${backend}.pid"
    )
    local pid
    pid=$(cat "/tmp/pds-${role}-${backend}.pid")
    echo "[pb-instance] launched role=$role  pid=$pid  log=$log_path"
}

# -----------------------------------------------------------------------------
# pb_wait_for_ready <role>
# Probe describeServer until 200 or timeout. Bounded retry: 60 attempts
# at 1s = 60s wall clock — enough headroom for first-launch crate-compile
# AND postgres-container handshake; the typical wait on a warm cache is
# under 5 attempts.
# -----------------------------------------------------------------------------

pb_wait_for_ready() {
    local role="$1"
    local upper
    upper=$(echo "$role" | tr '[:lower:]' '[:upper:]')
    local port_var="${upper}_PORT"
    local port="${!port_var}"
    local backend="${BACKEND:-sqlite}"
    local log_path="/tmp/pds-${role}-${backend}.log"
    local url="http://localhost:${port}/xrpc/com.atproto.server.describeServer"
    local i

    for i in $(seq 1 60); do
        if curl -sf "$url" >/dev/null 2>&1; then
            echo "[pb-instance] ready role=$role  port=$port  attempts=$i"
            return 0
        fi
        sleep 1
    done

    echo "[pb-instance] role=$role NOT READY after 60s — dumping last 50 log lines:" >&2
    tail -n 50 "$log_path" >&2 || true
    return 1
}

# -----------------------------------------------------------------------------
# pb_grep_banner <role>
# Confirm the launched instance's startup banner has the right port + the
# right data dir. Defense against launching with the wrong env file (e.g.
# A_ENV stale from a prior scenario).
# -----------------------------------------------------------------------------

pb_grep_banner() {
    local role="$1"
    local upper
    upper=$(echo "$role" | tr '[:lower:]' '[:upper:]')
    local port_var="${upper}_PORT"
    local data_var="${upper}_DATA"
    local port="${!port_var}"
    local data="${!data_var}"
    local backend="${BACKEND:-sqlite}"
    local log_path="/tmp/pds-${role}-${backend}.log"

    # Bare-word greps — Pretty subscriber inserts ANSI; a quoted
    # `port=2583` pattern would miss `port\033[…m=\033[…m2583`.
    # Match digits / paths separately.
    if ! grep -q "$port" "$log_path"; then
        echo "[pb-instance] banner check FAILED for role=$role: expected port=$port not in startup log" >&2
        return 1
    fi
    if ! grep -q "$data" "$log_path"; then
        echo "[pb-instance] banner check FAILED for role=$role: expected data=$data not in startup log" >&2
        return 1
    fi
    echo "[pb-instance] banner ok role=$role  port=$port  data=$data"
}

# -----------------------------------------------------------------------------
# pb_kill_instance <role>
# Graceful kill of one role; pid file lives in /tmp/pds-<role>-<backend>.pid.
# Best-effort; absent pid file is treated as no-op.
# -----------------------------------------------------------------------------

pb_kill_instance() {
    local role="$1"
    local backend="${BACKEND:-sqlite}"
    local pid_path="/tmp/pds-${role}-${backend}.pid"

    if [ -r "$pid_path" ]; then
        local pid
        pid=$(cat "$pid_path")
        if kill "$pid" 2>/dev/null; then
            echo "[pb-instance] killed role=$role pid=$pid"
        fi
        rm -f "$pid_path"
    fi
}

:
