# phase-b/lib/env.sh — source-once env helpers for Phase B scenarios.
#
# Source this file (don't exec it); it sets and exports env into the
# calling shell. Idempotent across re-sources so a block can re-assert
# its env after a terminal restart or sub-shell loss.
#
# Reads $BACKEND ("sqlite" | "postgres"; default "sqlite") to derive
# per-backend defaults: database URLs, container handles, ready-probe
# variant. Each scenario script is expected to call:
#
#   pb_env_init           # set BACKEND default + per-backend defaults
#   pb_env_emit_role a    # write /tmp/aurora-pb-env-a.sh and export A_ENV
#   pb_env_emit_role b    # write /tmp/aurora-pb-env-b.sh and export B_ENV
#   pb_env_echo_confirm   # echo the load-bearing vars for operator visibility
#
# Then re-source $A_ENV / $B_ENV at the entry of every block (env-drift
# guard — don't rely on process-global env carrying forward across a
# terminal restart or job-control surprise).
#
# Setup-only-never-judgment: this helper writes env files and echoes
# values. It does NOT decide pass/fail; the operator does.

set -u

# -----------------------------------------------------------------------------
# Per-scenario root + per-instance ports + data dirs.
# Scenarios may override A_PORT/B_PORT/A_DATA/B_DATA before calling
# pb_env_init; the defaults below are what the existing v0.5 markdown
# (arc12 / arc16c-f / arc17) settled on.
# -----------------------------------------------------------------------------

: "${A_PORT:=2583}"
: "${B_PORT:=2584}"
: "${A_DATA:=/tmp/aurora-phase-b/a-data}"
: "${B_DATA:=/tmp/aurora-phase-b/b-data}"

# Mock PLC port — shared default. Override only if a scenario reserves a
# different port (and arrange a corresponding lib/mock-plc.sh launch on
# that port).
: "${MOCK_PLC_PORT:=2582}"

# -----------------------------------------------------------------------------
# Backend selection.
# -----------------------------------------------------------------------------

pb_env_init() {
    : "${BACKEND:=sqlite}"
    export BACKEND

    case "$BACKEND" in
    sqlite)
        # Per-instance isolation comes from PDS_DATA_DIRECTORY. For SQLite
        # the PDS treats PDS_DB_URL as a *bare filesystem path*; the
        # production AnyPool layer (src/db/mod.rs::any_url_for) wraps it
        # internally as `sqlite://<path>?mode=rwc`. Emitting the URL form
        # here (as the v0.5 arc16f/arc17 markdown convention did) yields
        # `sqlite://sqlite:///<path>?mode=rwc?mode=rwc` after wrapping and
        # SQLx then rejects `mode=rwc?mode=rwc`. Bare path is the form the
        # config layer expects. Postgres branch still emits the full URL
        # because the PDS hands postgres URLs to sqlx untransformed.
        export A_DB_URL="$A_DATA/account.sqlite"
        export B_DB_URL="$B_DATA/account.sqlite"
        export PB_DB_BACKEND_VAL="sqlite"
        ;;
    postgres|pg)
        # Two containers on distinct ports per the v0.5 Postgres re-run
        # convention. Container provisioning is operator-side (the
        # harness assumes containers already running on 5432 + 5433);
        # see the scenario's setup block.
        export A_DB_URL="postgres://aurora:aurora@localhost:5432/aurora"
        export B_DB_URL="postgres://aurora:aurora@localhost:5433/aurora"
        export PB_DB_BACKEND_VAL="postgres"
        # Normalize the alias.
        export BACKEND="postgres"
        ;;
    *)
        echo "[pb-env] unsupported BACKEND='$BACKEND' (expected: sqlite | postgres)" >&2
        return 1
        ;;
    esac

    # Mock-PLC URL — shared default; scenarios that need a different
    # PLC URL override before calling pb_env_init.
    : "${PDS_DID_PLC_URL:=http://localhost:$MOCK_PLC_PORT}"
    export PDS_DID_PLC_URL

    # Sanity-echo on init (gives the operator a single line confirming
    # the resolved per-backend defaults before any env file is written).
    echo "[pb-env] init  BACKEND=$BACKEND  PLC=$PDS_DID_PLC_URL  A_PORT=$A_PORT  B_PORT=$B_PORT"
}

# -----------------------------------------------------------------------------
# Per-role env emission — writes /tmp/aurora-pb-env-<role>.sh and exports
# A_ENV / B_ENV (the path) into the calling shell.
#
# Role-specific overrides (LEXICON-HOST vs LEXICON-CONSUMER for Arc 17,
# federation flags for Arc 12, etc.) belong in the scenario script — set
# the env vars BEFORE calling pb_env_emit_role and they'll be picked up
# via the inheriting `cat > $env_path` heredoc.
# -----------------------------------------------------------------------------

pb_env_emit_role() {
    local role="$1"
    local upper
    upper=$(echo "$role" | tr '[:lower:]' '[:upper:]')

    local port_var="${upper}_PORT"
    local data_var="${upper}_DATA"
    local db_url_var="${upper}_DB_URL"
    local env_var="${upper}_ENV"

    local port="${!port_var}"
    local data="${!data_var}"
    local db_url="${!db_url_var}"
    local env_path="/tmp/aurora-pb-env-${role}.sh"

    # Optional per-role RUST_LOG default (debug for the federation
    # targets the v0.5 Phase B convention wanted; scenarios can
    # override before calling).
    : "${PB_RUST_LOG:=aurora_locus::federation::lexicon_resolver=debug,aurora_locus::federation::lexicon_fetcher_prod=debug,aurora_locus=info,tower_http=debug}"

    # Optional pass-through for scenario-set overrides — written ONLY
    # if set. Listed here so the heredoc consults the live env at
    # emit-time.
    local lexicon_enabled="${PDS_LEXICON_ENABLED:-}"
    local lexicon_did_authority="${PDS_LEXICON_DID_AUTHORITY:-}"
    local lexicon_fetch_failure_behavior="${PDS_LEXICON_FETCH_FAILURE_BEHAVIOR:-}"
    local lexicon_fetch_timeout_secs="${PDS_LEXICON_FETCH_TIMEOUT_SECS:-}"
    local federation_enabled="${PDS_FEDERATION_ENABLED:-}"

    cat > "$env_path" <<EOF
# Generated by phase-b/lib/env.sh — do not hand-edit; re-emit instead.
# Sourced at every block entry to guard against env-drift.
export PDS_SERVICE_HOSTNAME=localhost
export PDS_SERVICE_PORT=$port
export PDS_HOSTNAME=localhost
# PDS_PORT is the TCP bind port (src/config.rs:1608, default 2583).
# Distinct from PDS_SERVICE_PORT / PDS_PUBLIC_URL / PDS_SERVICE_DID
# which feed the public-URL + service-DID derivation. Without this
# explicit per-role bind-port, every instance binds 2583 regardless
# of A_PORT / B_PORT — role=a happens to work (A_PORT default is
# 2583) but role=b collides with role=a on 2583 and never listens
# on B_PORT, which is what the dual-backend scenarios assume.
export PDS_PORT=$port
export PDS_PUBLIC_URL=http://localhost:$port
export PDS_SERVICE_DID=did:web:localhost%3A$port
export PDS_SERVICE_HANDLE_DOMAINS=.localhost
export PDS_DATA_DIRECTORY=$data
export PDS_DID_PLC_URL=$PDS_DID_PLC_URL
export PDS_DB_BACKEND=$PB_DB_BACKEND_VAL
export PDS_DB_URL=$db_url
export PDS_INVITE_REQUIRED=false
export PDS_GC_SWEEP_ENABLED=false
export PDS_GC_SWEEP_ROW_SWEEP_ENABLED=false
export PDS_JWT_SECRET=\${PDS_JWT_SECRET:-pb-jwt-secret-$role-static-for-replayability-of-issued-tokens}
export PDS_ADMIN_PASSWORD=\${PDS_ADMIN_PASSWORD:-pb-admin-static-$role}
export PDS_REPO_SIGNING_KEY_K256_PRIVATE_KEY_HEX=\${PDS_REPO_SIGNING_KEY_K256_PRIVATE_KEY_HEX:-$(openssl rand -hex 32)}
export PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX=\${PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX:-$(openssl rand -hex 32)}
export RUST_LOG="$PB_RUST_LOG"
EOF

    # Append scenario-set overrides ONLY if they were set in the
    # calling shell — keeps the env files lean and lets scenarios
    # opt into per-scenario knobs without poisoning the defaults.
    [ -n "$lexicon_enabled" ] && echo "export PDS_LEXICON_ENABLED=$lexicon_enabled" >> "$env_path"
    [ -n "$lexicon_did_authority" ] && echo "export PDS_LEXICON_DID_AUTHORITY=$lexicon_did_authority" >> "$env_path"
    [ -n "$lexicon_fetch_failure_behavior" ] && echo "export PDS_LEXICON_FETCH_FAILURE_BEHAVIOR=$lexicon_fetch_failure_behavior" >> "$env_path"
    [ -n "$lexicon_fetch_timeout_secs" ] && echo "export PDS_LEXICON_FETCH_TIMEOUT_SECS=$lexicon_fetch_timeout_secs" >> "$env_path"
    [ -n "$federation_enabled" ] && echo "export PDS_FEDERATION_ENABLED=$federation_enabled" >> "$env_path"

    export "$env_var=$env_path"

    echo "[pb-env] wrote $env_path  (role=$role port=$port data=$data db=$PB_DB_BACKEND_VAL)"
}

# -----------------------------------------------------------------------------
# pb_env_echo_confirm — operator-visible vars at block entry.
# Bake this into every block so the operator sees the env in use before
# the block does anything with it (env-drift guard).
# -----------------------------------------------------------------------------

pb_env_echo_confirm() {
    echo "[pb-env] BACKEND=${BACKEND:-<unset>}  A_PORT=${A_PORT:-<unset>}  A_DATA=${A_DATA:-<unset>}  B_PORT=${B_PORT:-<unset>}  B_DATA=${B_DATA:-<unset>}"
    echo "[pb-env] PLC=${PDS_DID_PLC_URL:-<unset>}  A_ENV=${A_ENV:-<unset>}  B_ENV=${B_ENV:-<unset>}"
}

# Re-source guards: idempotent if a scenario re-sources lib/env.sh
# after re-emitting per-role env files.
:
