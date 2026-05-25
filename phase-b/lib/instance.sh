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
# pb_pg_provision <role>
# Ensure the role's Postgres container is running + reachable on the
# host:port env.sh emitted for the role. Auto-starts the container if
# absent (docker run with the conventional name/port/creds) or restarts
# it if stopped (docker start). Idempotent: a no-op fast probe when the
# container is already up.
#
# Lifecycle is harness-managed under BACKEND=postgres, mirroring the
# SQLite path's data-dir auto-provisioning. Schema isolation is
# orthogonal: pb_fresh_data_dir (in lib/data.sh) wipes both the data
# dir AND the role's PG schema when BACKEND=postgres, so scenarios that
# want clean state get it regardless of backend.
#
# Degraded fail-fast: if docker isn't available (or the daemon refuses
# the start) we fall back to a clear error message + the exact docker
# incantation, the same shape the d511291 preflight printed. The
# operator sees a bounded actionable error instead of a 60s
# PoolTimedOut spin.
#
# Reads the role's host+port from ${ROLE}_DB_URL (set by pb_env_init's
# postgres branch) so the probe stays in lockstep with whatever URL the
# launched PDS will actually try to connect to.
# -----------------------------------------------------------------------------

pb_pg_provision() {
    local role="$1"
    local upper
    upper=$(echo "$role" | tr '[:lower:]' '[:upper:]')
    local url_var="${upper}_DB_URL"
    local url="${!url_var:-}"

    if [ -z "$url" ]; then
        echo "[pb-instance] pg provision: $url_var unset — pb_env_init must run before pb_pg_provision under BACKEND=postgres" >&2
        return 1
    fi

    # Extract host + port from `postgres://user:pass@host:port/db`.
    local hostport="${url#*@}"
    hostport="${hostport%%/*}"
    local host="${hostport%%:*}"
    local port="${hostport##*:}"

    local container="aurora-phase-b-pg-${role}"

    # ----- Phase 1: probe. -----
    # Already reachable -> done, no-op.
    if _pb_pg_probe "$host" "$port"; then
        echo "[pb-instance] pg provision ok role=$role  container=$container  $host:$port  (already up)"
        return 0
    fi

    # ----- Phase 2: try to bring the container up via docker. -----
    if ! command -v docker >/dev/null 2>&1; then
        _pb_pg_fail_fast "$container" "$host" "$port" "docker not on PATH"
        return 1
    fi

    # docker inspect tells us whether the container exists + its state.
    # `2>/dev/null` swallows the "no such object" error so we can branch
    # on the empty result.
    local existing_state
    existing_state=$(docker inspect -f '{{.State.Running}}' "$container" 2>/dev/null || true)

    case "$existing_state" in
    true)
        # Container says it's running but the probe just failed —
        # likely starting up or stuck. Fall through to the wait loop.
        echo "[pb-instance] pg provision: $container reports running but probe failed, waiting"
        ;;
    false)
        echo "[pb-instance] pg provision: starting existing stopped container $container"
        if ! docker start "$container" >/dev/null 2>&1; then
            _pb_pg_fail_fast "$container" "$host" "$port" "docker start failed"
            return 1
        fi
        ;;
    "")
        echo "[pb-instance] pg provision: creating $container on $host:$port"
        if ! docker run -d --name "$container" \
            -p "${port}:5432" \
            -e POSTGRES_USER=aurora -e POSTGRES_PASSWORD=aurora \
            -e POSTGRES_DB=aurora \
            postgres:16 >/dev/null 2>&1; then
            _pb_pg_fail_fast "$container" "$host" "$port" "docker run failed"
            return 1
        fi
        ;;
    *)
        echo "[pb-instance] pg provision: docker inspect returned unexpected state '$existing_state' for $container" >&2
        return 1
        ;;
    esac

    # ----- Phase 3: bounded wait for ready. -----
    # postgres:16 cold-start to accepting connections is typically
    # 2-5s; bound generously at 30s to absorb slow disks.
    local i
    for i in $(seq 1 30); do
        if _pb_pg_probe "$host" "$port"; then
            echo "[pb-instance] pg provision ok role=$role  container=$container  $host:$port  (ready in ${i}s)"
            return 0
        fi
        sleep 1
    done

    echo "[pb-instance] pg provision: $container started but not reachable on $host:$port within 30s" >&2
    echo "[pb-instance] container logs (last 30 lines):" >&2
    docker logs --tail 30 "$container" >&2 2>&1 || true
    return 1
}

# -----------------------------------------------------------------------------
# _pb_pg_probe <host> <port>
# Internal: returns 0 if the postgres TCP endpoint is responding.
# Prefers pg_isready when on PATH (recognizes server liveness, not just
# TCP); falls back to bash's /dev/tcp probe so a fresh operator without
# postgresql-client still gets a clean signal.
# -----------------------------------------------------------------------------

_pb_pg_probe() {
    local host="$1"
    local port="$2"
    if command -v pg_isready >/dev/null 2>&1; then
        pg_isready -h "$host" -p "$port" -q >/dev/null 2>&1
    else
        # `timeout 2` prevents a stale half-open port from hanging on
        # SYN_SENT.
        timeout 2 bash -c "exec 9<>/dev/tcp/${host}/${port}" 2>/dev/null
    fi
}

# -----------------------------------------------------------------------------
# _pb_pg_fail_fast <container> <host> <port> <reason>
# Internal: print the degraded-mode fail-fast message + the docker
# incantation the operator can run by hand. Preserved from the d511291
# preflight so docker-absent / can't-start environments still get a
# bounded actionable error instead of PoolTimedOut.
# -----------------------------------------------------------------------------

_pb_pg_fail_fast() {
    local container="$1"
    local host="$2"
    local port="$3"
    local reason="$4"
    echo "[pb-instance] postgres container $container not reachable on $host:$port ($reason)" >&2
    echo "[pb-instance] auto-provision is unavailable; start it manually:" >&2
    echo "  docker run -d --name $container -p ${port}:5432 \\" >&2
    echo "    -e POSTGRES_USER=aurora -e POSTGRES_PASSWORD=aurora \\" >&2
    echo "    -e POSTGRES_DB=aurora postgres:16" >&2
}

# -----------------------------------------------------------------------------
# pb_launch_instance <role>
# Expects role-env already emitted (A_ENV/B_ENV path set + file readable)
# and the per-role data dir pre-created (call lib/data.sh helpers first).
# Logs to /tmp/pds-<role>-<backend>.log (redirect, NOT tee); log file is
# truncated at launch so failed-launch tails can't show a prior run's
# banner.
# Under BACKEND=postgres, pb_pg_provision ensures the role's container is
# up (auto-start if absent, restart if stopped) before launching the PDS.
# Schema isolation is orthogonal — pb_fresh_data_dir (lib/data.sh) is the
# scenario-driven wipe that mirrors SQLite's fresh-data-dir.
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

    # Postgres auto-provision: ensure the role's container is up
    # (start if absent, restart if stopped, no-op if up) and reachable.
    # Without this, a missing container caused the PDS to spin to
    # PoolTimedOut after 60s with a generic error. SQLite path skips
    # this branch entirely — it self-provisions its data dir via
    # pb_fresh_data_dir.
    if [ "$backend" = "postgres" ]; then
        pb_pg_provision "$role" || return 1
    fi

    # Truncate the log so a failed launch can't surface a previous
    # run's banner as if it were this run's. Without this, a void
    # launch's "dump last 50 lines" tail prints a stale-but-plausible
    # PDS-up log from the prior run and looks like a successful start.
    : > "$log_path"

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
