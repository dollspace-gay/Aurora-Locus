# phase-b/lib/data.sh — fresh-data-dir helpers for per-scenario isolation.
#
# Source this file. Provides:
#
#   pb_fresh_data_dir <role>     # rm -rf + mkdir for the role's data dir;
#                                #   ALSO wipes the role's PG schema when
#                                #   BACKEND=postgres (backend-symmetric
#                                #   clean-state primitive).
#   pb_pg_fresh_schema <role>    # `DROP SCHEMA public CASCADE; CREATE
#                                #   SCHEMA public AUTHORIZATION aurora;`
#                                #   inside the role's container. Used
#                                #   internally by pb_fresh_data_dir; can
#                                #   also be called directly if a scenario
#                                #   wants to wipe ONLY the DB.
#   pb_fresh_all                 # both roles + the shared root
#   pb_purge_legacy              # remove the v0.5 hardcoded per-arc data dirs
#
# Why fresh-not-soft-reset: content-addressed IDs (record CIDs, blob
# CIDs, commit CIDs) are deterministic across runs given identical
# inputs. A soft-reset that re-uses the data dir cross-contaminates
# CID sets across scenarios — Phase B then exercises the wrong
# preconditions (see memory: feedback_phase_b_state_isolation).
#
# Backend symmetry: under SQLite the data dir IS the database (the
# account.sqlite file lives inside it), so rm-rf+mkdir is the full
# clean-state guarantee. Under Postgres the database lives in a
# container outside the data dir, so a parallel schema wipe is needed
# to match. pb_fresh_data_dir does both transparently — scenarios call
# it the same way under either backend and get "clean slate for role X."
# This requires lib/instance.sh to also be sourced (for pb_pg_provision),
# which scenario scripts already do.
#
# Setup-only-never-judgment: these helpers create directories and wipe
# database state. They do NOT decide whether a scenario passed.

set -u

# -----------------------------------------------------------------------------
# pb_fresh_data_dir <role>
# Wipe (rm -rf) and recreate the role's data dir. Required between
# scenarios that assert side-effects against fresh state.
# -----------------------------------------------------------------------------

pb_fresh_data_dir() {
    local role="$1"
    local upper
    upper=$(echo "$role" | tr '[:lower:]' '[:upper:]')
    local data_var="${upper}_DATA"
    local data="${!data_var:-}"

    if [ -z "$data" ]; then
        echo "[pb-data] cannot fresh-data-dir role=$role: ${data_var} is unset" >&2
        return 1
    fi
    # Guard rail: refuse if the path looks dangerous (root, home, /etc).
    case "$data" in
    /|/home|/home/*|/etc|/etc/*|/usr|/usr/*|/var|/var/*)
        echo "[pb-data] refusing to wipe '$data' (looks like a system path)" >&2
        return 1
        ;;
    esac

    rm -rf "$data"
    mkdir -p "$data"
    echo "[pb-data] fresh role=$role  path=$data"

    # Backend symmetry: under Postgres the schema lives in a container,
    # not in the data dir, so a parallel wipe is needed. pb_pg_provision
    # ensures the container is up before the wipe (auto-start if absent,
    # fast probe-only no-op if already running) so scenarios can call
    # pb_fresh_data_dir without ordering it after pb_launch_instance.
    if [ "${BACKEND:-sqlite}" = "postgres" ]; then
        pb_pg_provision "$role" || return 1
        pb_pg_fresh_schema "$role" || return 1
    fi
}

# -----------------------------------------------------------------------------
# pb_pg_fresh_schema <role>
# Wipe the role's Postgres database state in place. Drops the public
# schema (which holds all PDS-owned tables AND sqlx's _sqlx_migrations
# ledger) and recreates it owned by aurora. The PDS re-runs all
# migrations from scratch on next launch via run_any_migrations
# (src/db/mod.rs::run_any_migrations). Safe because the PDS pg
# migrations are pure tables/indexes — no extensions, no non-public
# schemas, no functions outside public (audited as of migrations/
# postgres/0001-0005), so DROP SCHEMA public CASCADE is a complete
# clean-state.
#
# Requires the role's container to be running (call pb_pg_provision
# first, or use pb_fresh_data_dir which does it for you). Uses
# `docker exec` rather than host psql so the operator doesn't need
# postgresql-client installed.
# -----------------------------------------------------------------------------

pb_pg_fresh_schema() {
    local role="$1"
    local container="aurora-phase-b-pg-${role}"
    if ! docker exec "$container" psql -U aurora -d aurora -qAtv ON_ERROR_STOP=1 -c \
        'DROP SCHEMA IF EXISTS public CASCADE; CREATE SCHEMA public AUTHORIZATION aurora;' \
        >/dev/null 2>&1; then
        echo "[pb-data] pg fresh-schema FAILED for role=$role container=$container" >&2
        return 1
    fi
    echo "[pb-data] pg fresh-schema role=$role  container=$container"
}

# -----------------------------------------------------------------------------
# pb_fresh_all — wipe both A and B data dirs + the shared root.
# Common at scenario start (Setup-to-confirmed-up block).
# -----------------------------------------------------------------------------

pb_fresh_all() {
    pb_fresh_data_dir a
    pb_fresh_data_dir b

    # Shared root (/tmp/aurora-phase-b) — wipe only the role children;
    # leave the root in place so re-runs don't recreate it from scratch.
    :
}

# -----------------------------------------------------------------------------
# pb_purge_legacy — remove the v0.5 hardcoded per-arc data dirs the
# operator-driven markdown left behind (e.g. /tmp/aurora-arc17-phase-b/*).
# Optional; helps avoid confusion between old markdown-driven runs and
# the new harness layout.
# -----------------------------------------------------------------------------

pb_purge_legacy() {
    local stale
    for stale in /tmp/aurora-arc12-phase-b /tmp/aurora-arc13-phase-b /tmp/aurora-arc14-phase-b /tmp/aurora-arc15-phase-b /tmp/aurora-arc16c-phase-b /tmp/aurora-arc16d-phase-b /tmp/aurora-arc16e-phase-b /tmp/aurora-arc16f-phase-b /tmp/aurora-arc17-phase-b; do
        if [ -d "$stale" ]; then
            rm -rf "$stale"
            echo "[pb-data] purged legacy $stale"
        fi
    done
}

:
