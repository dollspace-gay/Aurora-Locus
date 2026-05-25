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
# pb_pg_preflight <role>
# Probe the operator-managed Postgres container for $role and fail fast
# with a clear "start the container first" message if absent. Without
# this, a missing container causes the PDS to spin on sqlx's connect
# retry loop until PoolTimedOut (~60s) with a generic error that hides
# the real cause. The probe prefers `pg_isready` when available; falls
# back to bash's `/dev/tcp` TCP-handshake check so a fresh operator
# environment without `postgresql-client` still gets a clean signal.
# Reads the role's port from the URL env.sh emitted (A_DB_URL / B_DB_URL)
# so the preflight stays in lockstep with whatever URL the launched PDS
# would actually try to connect to.
# -----------------------------------------------------------------------------

pb_pg_preflight() {
    local role="$1"
    local upper
    upper=$(echo "$role" | tr '[:lower:]' '[:upper:]')
    local url_var="${upper}_DB_URL"
    local url="${!url_var:-}"

    if [ -z "$url" ]; then
        echo "[pb-instance] pg preflight: $url_var unset — pb_env_init must run before pb_launch_instance under BACKEND=postgres" >&2
        return 1
    fi

    # Extract host + port from `postgres://user:pass@host:port/db`.
    # Strip the scheme + auth, leave `host:port/db`, then split.
    local hostport="${url#*@}"
    hostport="${hostport%%/*}"
    local host="${hostport%%:*}"
    local port="${hostport##*:}"

    local container="aurora-phase-b-pg-${role}"

    # Prefer pg_isready (recognizes server liveness, not just TCP).
    # Fall back to bash's /dev/tcp probe so the operator doesn't need
    # postgresql-client installed just to get a fail-fast signal.
    local probe_ok=1
    if command -v pg_isready >/dev/null 2>&1; then
        if pg_isready -h "$host" -p "$port" -q >/dev/null 2>&1; then
            probe_ok=0
        fi
    else
        # /dev/tcp is bash-native; the script is bash-only so this is
        # always available. Wrapping in `timeout 2` keeps a stale port
        # from hanging on SYN_SENT.
        if timeout 2 bash -c "exec 9<>/dev/tcp/${host}/${port}" 2>/dev/null; then
            probe_ok=0
        fi
    fi

    if [ "$probe_ok" -ne 0 ]; then
        echo "[pb-instance] postgres container ${container} not reachable on ${host}:${port}" >&2
        echo "[pb-instance] start it first (see phase-b/README.md \"Postgres prerequisite\"):" >&2
        echo "  docker run -d --name ${container} -p ${port}:5432 \\" >&2
        echo "    -e POSTGRES_USER=aurora -e POSTGRES_PASSWORD=aurora \\" >&2
        echo "    -e POSTGRES_DB=aurora postgres:16" >&2
        return 1
    fi

    echo "[pb-instance] pg preflight ok role=$role  container=${container}  ${host}:${port}"
}

# -----------------------------------------------------------------------------
# pb_launch_instance <role>
# Expects role-env already emitted (A_ENV/B_ENV path set + file readable)
# and the per-role data dir pre-created (call lib/data.sh helpers first).
# Logs to /tmp/pds-<role>-<backend>.log (redirect, NOT tee).
# Under BACKEND=postgres, pb_pg_preflight gates the launch so a missing
# operator-managed container surfaces fast instead of as PoolTimedOut.
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

    # Postgres preflight: fail fast with the docker incantation if the
    # operator-managed container isn't reachable, rather than letting
    # sqlx burn 60s to PoolTimedOut. See phase-b/README.md "Postgres
    # prerequisite". Previously this preflight lived only in scenario-11
    # (the matrix wrapper); promoted to instance.sh so every scenario
    # picks it up.
    if [ "$backend" = "postgres" ]; then
        pb_pg_preflight "$role" || return 1
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
